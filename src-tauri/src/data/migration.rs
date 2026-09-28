use std::{error::Error, fmt};

use serde_json::Value;

use super::compatibility::{
    inspect_schema_compatibility, SchemaCompatibility, SchemaCompatibilityErrorCategory,
    SchemaSupportPolicy,
};
use super::json::{parse_strict_json_object, to_deterministic_json_bytes, StrictJsonErrorCategory};
use super::schema::{SchemaVersion, CURRENT_SCHEMA_VERSION};

const SCHEMA_VERSION_FIELD: &str = "schemaVersion";

pub(crate) type MigrationTransform = fn(Value) -> Result<Value, MigrationStepError>;
pub(crate) type MigrationValidator = fn(&Value) -> Result<(), MigrationStepError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MigrationTransition {
    pub(crate) from: SchemaVersion,
    pub(crate) to: SchemaVersion,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MigrationStep {
    from: u32,
    to: u32,
    transform: MigrationTransform,
    validate_input: MigrationValidator,
    validate_output: MigrationValidator,
}

impl MigrationStep {
    pub(crate) const fn new(
        from: u32,
        to: u32,
        transform: MigrationTransform,
        validate_input: MigrationValidator,
        validate_output: MigrationValidator,
    ) -> Self {
        Self {
            from,
            to,
            transform,
            validate_input,
            validate_output,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MigrationRegistry<'steps> {
    current: SchemaVersion,
    minimum_migratable: SchemaVersion,
    steps: &'steps [MigrationStep],
}

impl<'steps> MigrationRegistry<'steps> {
    pub(crate) fn try_new(
        current: u32,
        steps: &'steps [MigrationStep],
    ) -> Result<Self, MigrationError> {
        let current = SchemaVersion::try_from(current)
            .map_err(|_| MigrationError::registry("current version must be positive"))?;

        let mut previous_to = None;
        for step in steps {
            let from = SchemaVersion::try_from(step.from)
                .map_err(|_| MigrationError::registry("step source version must be positive"))?;
            let to = SchemaVersion::try_from(step.to)
                .map_err(|_| MigrationError::registry("step target version must be positive"))?;
            if step.from.checked_add(1) != Some(step.to) {
                return Err(MigrationError::registry(
                    "each migration step must advance exactly one version",
                ));
            }
            if to > current {
                return Err(MigrationError::registry(
                    "migration step must not exceed the current version",
                ));
            }
            if previous_to.is_some_and(|previous| previous != from) {
                return Err(MigrationError::registry(
                    "migration steps must form one ordered chain without gaps or duplicates",
                ));
            }
            previous_to = Some(to);
        }
        if previous_to.is_some_and(|last| last != current) {
            return Err(MigrationError::registry(
                "migration chain must end at the current version",
            ));
        }

        let minimum_migratable = steps
            .first()
            .map(|step| SchemaVersion::try_from(step.from))
            .transpose()
            .map_err(|_| MigrationError::registry("step source version must be positive"))?
            .unwrap_or(current);
        Ok(Self {
            current,
            minimum_migratable,
            steps,
        })
    }

    pub(crate) fn current(self) -> SchemaVersion {
        self.current
    }

    pub(crate) fn minimum_migratable(self) -> SchemaVersion {
        self.minimum_migratable
    }

    pub(crate) fn support_policy(self) -> Result<SchemaSupportPolicy, MigrationError> {
        Ok(SchemaSupportPolicy::from_validated_registry(
            ValidatedRegistrySupport {
                current: self.current,
                minimum_migratable: self.minimum_migratable,
            },
        ))
    }

    pub(crate) fn plan(
        self,
        source: SchemaVersion,
    ) -> Result<MigrationPlan<'steps>, MigrationError> {
        if source == self.current {
            return Err(MigrationError::context(
                MigrationErrorCategory::MigrationNotRequired,
                MigrationStage::PlanCreation,
                "source is already the current version",
                Some(source),
                Some(self.current),
                None,
            ));
        }
        if source > self.current {
            return Err(MigrationError::context(
                MigrationErrorCategory::UnsupportedFuture,
                MigrationStage::PlanCreation,
                "future project schema cannot be migrated",
                Some(source),
                Some(self.current),
                None,
            ));
        }
        if source < self.minimum_migratable {
            return Err(MigrationError::context(
                MigrationErrorCategory::UnsupportedPast,
                MigrationStage::PlanCreation,
                "project schema is older than the migration chain",
                Some(source),
                Some(self.current),
                None,
            ));
        }
        let start = self
            .steps
            .iter()
            .position(|step| step.from == source.get())
            .ok_or_else(|| {
                MigrationError::context(
                    MigrationErrorCategory::PlanUnavailable,
                    MigrationStage::PlanCreation,
                    "no exact migration step starts at the source version",
                    Some(source),
                    Some(self.current),
                    None,
                )
            })?;
        Ok(MigrationPlan {
            source,
            target: self.current,
            steps: &self.steps[start..],
        })
    }
}

/// 필드가 비공개이므로 검증된 `MigrationRegistry`만 production 정책 증표를 만들 수 있다.
pub(super) struct ValidatedRegistrySupport {
    current: SchemaVersion,
    minimum_migratable: SchemaVersion,
}

impl ValidatedRegistrySupport {
    pub(super) const fn current(&self) -> SchemaVersion {
        self.current
    }

    pub(super) const fn minimum_migratable(&self) -> SchemaVersion {
        self.minimum_migratable
    }
}

static PRODUCTION_STEPS: [MigrationStep; 0] = [];

pub(crate) fn production_registry() -> Result<MigrationRegistry<'static>, MigrationError> {
    MigrationRegistry::try_new(CURRENT_SCHEMA_VERSION.get(), &PRODUCTION_STEPS)
}

#[derive(Debug)]
pub(crate) enum MigrationDecision<'steps> {
    Current { version: SchemaVersion },
    Required(MigrationPlan<'steps>),
}

pub(crate) fn inspect_migration<'steps>(
    bytes: &[u8],
    registry: MigrationRegistry<'steps>,
) -> Result<MigrationDecision<'steps>, MigrationError> {
    let policy = registry.support_policy()?;
    match inspect_schema_compatibility(bytes, policy).map_err(|source| {
        let category =
            if source.category() == SchemaCompatibilityErrorCategory::DuplicateSchemaVersion {
                MigrationErrorCategory::DuplicateJsonKey
            } else {
                MigrationErrorCategory::InvalidInputJson
            };
        MigrationError::with_source(
            category,
            MigrationStage::CompatibilityInspection,
            "project header is invalid",
            None,
            Some(registry.current),
            None,
            source,
        )
    })? {
        SchemaCompatibility::Current { version } => Ok(MigrationDecision::Current { version }),
        SchemaCompatibility::MigrationRequired { found, .. } => {
            registry.plan(found).map(MigrationDecision::Required)
        }
        SchemaCompatibility::UnsupportedFuture { found, current } => Err(MigrationError::context(
            MigrationErrorCategory::UnsupportedFuture,
            MigrationStage::CompatibilityInspection,
            "future project schema cannot be migrated",
            Some(found),
            Some(current),
            None,
        )),
        SchemaCompatibility::UnsupportedPast { found, .. } => Err(MigrationError::context(
            MigrationErrorCategory::UnsupportedPast,
            MigrationStage::CompatibilityInspection,
            "project schema is outside the migration range",
            Some(found),
            Some(registry.current),
            None,
        )),
    }
}

#[derive(Debug)]
pub(crate) struct MigrationPlan<'steps> {
    source: SchemaVersion,
    target: SchemaVersion,
    steps: &'steps [MigrationStep],
}

impl MigrationPlan<'_> {
    pub(crate) fn execute(&self, bytes: &[u8]) -> Result<MigrationResult, MigrationError> {
        let mut value = parse_strict_json(bytes, MigrationStage::InputParsing)?;
        let found = value_schema_version(&value).ok_or_else(|| {
            MigrationError::context(
                MigrationErrorCategory::InputVersionMismatch,
                MigrationStage::InputValidation,
                "input schema version is missing or invalid",
                None,
                Some(self.source),
                None,
            )
        })?;
        if found != self.source {
            return Err(MigrationError::context(
                MigrationErrorCategory::InputVersionMismatch,
                MigrationStage::InputValidation,
                "input schema version differs from the migration plan",
                Some(found),
                Some(self.source),
                None,
            ));
        }

        let mut applied_steps = Vec::with_capacity(self.steps.len());
        for step in self.steps {
            let transition = transition(step);
            let current = value_schema_version(&value);
            if current != Some(transition.from) {
                return Err(MigrationError::context(
                    MigrationErrorCategory::InputVersionMismatch,
                    MigrationStage::InputValidation,
                    "step input schema version does not match its source",
                    current,
                    Some(transition.from),
                    Some(transition),
                ));
            }
            (step.validate_input)(&value).map_err(|source| {
                MigrationError::with_source(
                    MigrationErrorCategory::InputValidationFailure,
                    MigrationStage::InputValidation,
                    "migration step input validation failed",
                    current,
                    Some(transition.to),
                    Some(transition),
                    source,
                )
            })?;
            value = (step.transform)(value).map_err(|source| {
                MigrationError::with_source(
                    MigrationErrorCategory::StepFailure,
                    MigrationStage::Transform,
                    "migration step failed",
                    Some(transition.from),
                    Some(transition.to),
                    Some(transition),
                    source,
                )
            })?;
            let output_version = value_schema_version(&value);
            if output_version != Some(transition.to) {
                return Err(MigrationError::context(
                    MigrationErrorCategory::OutputVersionMismatch,
                    MigrationStage::OutputValidation,
                    "step output schema version does not match its target",
                    output_version,
                    Some(transition.to),
                    Some(transition),
                ));
            }
            (step.validate_output)(&value).map_err(|source| {
                MigrationError::with_source(
                    MigrationErrorCategory::OutputValidationFailure,
                    MigrationStage::OutputValidation,
                    "migration step output validation failed",
                    output_version,
                    Some(transition.to),
                    Some(transition),
                    source,
                )
            })?;
            applied_steps.push(transition);
        }

        if value_schema_version(&value) != Some(self.target) {
            return Err(MigrationError::context(
                MigrationErrorCategory::FinalVersionMismatch,
                MigrationStage::FinalValidation,
                "migration result did not reach the registry current version",
                value_schema_version(&value),
                Some(self.target),
                None,
            ));
        }
        let output = to_deterministic_json_bytes(&value).map_err(|source| {
            MigrationError::with_source(
                MigrationErrorCategory::SerializationFailure,
                MigrationStage::Serialization,
                "migration result could not be serialized",
                Some(self.source),
                Some(self.target),
                None,
                source,
            )
        })?;
        let reparsed =
            parse_strict_json(&output, MigrationStage::OutputRevalidation).map_err(|source| {
                MigrationError::with_source(
                    MigrationErrorCategory::DeterministicOutputRevalidationFailure,
                    MigrationStage::OutputRevalidation,
                    "deterministic migration output could not be revalidated",
                    Some(self.source),
                    Some(self.target),
                    None,
                    source,
                )
            })?;
        if value_schema_version(&reparsed) != Some(self.target) {
            return Err(MigrationError::context(
                MigrationErrorCategory::DeterministicOutputRevalidationFailure,
                MigrationStage::OutputRevalidation,
                "deterministic output has an unexpected schema version",
                value_schema_version(&reparsed),
                Some(self.target),
                None,
            ));
        }
        if let Some(last) = self.steps.last() {
            (last.validate_output)(&reparsed).map_err(|source| {
                MigrationError::with_source(
                    MigrationErrorCategory::DeterministicOutputRevalidationFailure,
                    MigrationStage::OutputRevalidation,
                    "deterministic output failed final schema validation",
                    Some(self.target),
                    Some(self.target),
                    Some(transition(last)),
                    source,
                )
            })?;
        }
        Ok(MigrationResult {
            source_version: self.source,
            final_version: self.target,
            bytes: output,
            applied_steps,
        })
    }
}

#[derive(Debug)]
pub(crate) struct MigrationResult {
    pub(crate) source_version: SchemaVersion,
    pub(crate) final_version: SchemaVersion,
    pub(crate) bytes: Vec<u8>,
    pub(crate) applied_steps: Vec<MigrationTransition>,
}

#[derive(Debug)]
pub(crate) struct MigrationStepError {
    detail: &'static str,
}

impl MigrationStepError {
    pub(crate) const fn new(detail: &'static str) -> Self {
        Self { detail }
    }
}

impl fmt::Display for MigrationStepError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.detail)
    }
}

impl Error for MigrationStepError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationErrorCategory {
    InvalidRegistry,
    MigrationNotRequired,
    PlanUnavailable,
    UnsupportedFuture,
    UnsupportedPast,
    InvalidInputJson,
    DuplicateJsonKey,
    InputVersionMismatch,
    InputValidationFailure,
    StepFailure,
    OutputValidationFailure,
    OutputVersionMismatch,
    FinalVersionMismatch,
    SerializationFailure,
    DeterministicOutputRevalidationFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationStage {
    RegistryValidation,
    CompatibilityInspection,
    PlanCreation,
    InputParsing,
    InputValidation,
    Transform,
    OutputValidation,
    FinalValidation,
    Serialization,
    OutputRevalidation,
}

pub(crate) struct MigrationError {
    category: MigrationErrorCategory,
    stage: MigrationStage,
    detail: &'static str,
    source_version: Option<SchemaVersion>,
    target_version: Option<SchemaVersion>,
    transition: Option<MigrationTransition>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl fmt::Debug for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // source의 Debug에는 parser가 본 실제 값이 들어갈 수 있어 안전한 문맥만 출력한다.
        formatter
            .debug_struct("MigrationError")
            .field("category", &self.category)
            .field("stage", &self.stage)
            .field("detail", &self.detail)
            .field("source_version", &self.source_version)
            .field("target_version", &self.target_version)
            .field("transition", &self.transition)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl MigrationError {
    pub(crate) fn category(&self) -> MigrationErrorCategory {
        self.category
    }

    /// 상위 batch 오류가 사용자 본문 없이 실패 step의 version 문맥을 보존하게 한다.
    pub(crate) fn context_versions(&self) -> (Option<SchemaVersion>, Option<SchemaVersion>) {
        self.transition
            .map(|transition| (Some(transition.from), Some(transition.to)))
            .unwrap_or((self.source_version, self.target_version))
    }

    fn registry(detail: &'static str) -> Self {
        Self::context(
            MigrationErrorCategory::InvalidRegistry,
            MigrationStage::RegistryValidation,
            detail,
            None,
            None,
            None,
        )
    }

    fn context(
        category: MigrationErrorCategory,
        stage: MigrationStage,
        detail: &'static str,
        source_version: Option<SchemaVersion>,
        target_version: Option<SchemaVersion>,
        transition: Option<MigrationTransition>,
    ) -> Self {
        Self {
            category,
            stage,
            detail,
            source_version,
            target_version,
            transition,
            source: None,
        }
    }

    fn with_source(
        category: MigrationErrorCategory,
        stage: MigrationStage,
        detail: &'static str,
        source_version: Option<SchemaVersion>,
        target_version: Option<SchemaVersion>,
        transition: Option<MigrationTransition>,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            category,
            stage,
            detail,
            source_version,
            target_version,
            transition,
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "project schema migration failed ({:?}) at {:?}",
            self.category, self.stage
        )?;
        if let Some(transition) = self.transition {
            write!(
                formatter,
                " for {} -> {}",
                transition.from.get(),
                transition.to.get()
            )?;
        } else if let (Some(source), Some(target)) = (self.source_version, self.target_version) {
            write!(formatter, " for {} -> {}", source.get(), target.get())?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl Error for MigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

fn transition(step: &MigrationStep) -> MigrationTransition {
    MigrationTransition {
        // MigrationPlan은 검증된 registry에서만 생성되므로 두 값은 양수다.
        from: SchemaVersion::new_unchecked(step.from),
        to: SchemaVersion::new_unchecked(step.to),
    }
}

fn value_schema_version(value: &Value) -> Option<SchemaVersion> {
    let raw = value.as_object()?.get(SCHEMA_VERSION_FIELD)?.as_u64()?;
    let raw = u32::try_from(raw).ok()?;
    SchemaVersion::try_from(raw).ok()
}

/// filesystem 재검증이 실제 reread bytes를 strict JSON으로 다시 확인할 때 사용한다.
pub(super) fn strict_schema_version(bytes: &[u8]) -> Result<SchemaVersion, MigrationError> {
    let value = parse_strict_json(bytes, MigrationStage::InputParsing)?;
    value_schema_version(&value).ok_or_else(|| {
        MigrationError::context(
            MigrationErrorCategory::InvalidInputJson,
            MigrationStage::InputParsing,
            "strict JSON requires a positive schemaVersion",
            None,
            None,
            None,
        )
    })
}

/// migration 결과가 strict JSON이며 기존 결정적 직렬화 형식과 정확히 일치하는지
/// batch 경계에서 다시 확인한다. 사용자 본문은 오류 문맥에 포함하지 않는다.
pub(super) fn validate_deterministic_migration_output(
    bytes: &[u8],
    expected_version: SchemaVersion,
) -> Result<(), MigrationError> {
    let value = parse_strict_json(bytes, MigrationStage::OutputRevalidation)?;
    if value_schema_version(&value) != Some(expected_version) {
        return Err(MigrationError::context(
            MigrationErrorCategory::FinalVersionMismatch,
            MigrationStage::OutputRevalidation,
            "deterministic output header does not match the expected version",
            Some(expected_version),
            Some(expected_version),
            None,
        ));
    }
    let deterministic = to_deterministic_json_bytes(&value).map_err(|source| {
        MigrationError::with_source(
            MigrationErrorCategory::SerializationFailure,
            MigrationStage::OutputRevalidation,
            "migration output could not be serialized deterministically",
            Some(expected_version),
            Some(expected_version),
            None,
            source,
        )
    })?;
    if deterministic != bytes {
        return Err(MigrationError::context(
            MigrationErrorCategory::DeterministicOutputRevalidationFailure,
            MigrationStage::OutputRevalidation,
            "migration output is not in the deterministic JSON format",
            Some(expected_version),
            Some(expected_version),
            None,
        ));
    }
    Ok(())
}

fn parse_strict_json(bytes: &[u8], stage: MigrationStage) -> Result<Value, MigrationError> {
    parse_strict_json_object(bytes).map_err(|source| {
        let category = if source.category() == StrictJsonErrorCategory::DuplicateKey {
            MigrationErrorCategory::DuplicateJsonKey
        } else {
            MigrationErrorCategory::InvalidInputJson
        };
        MigrationError::with_source(
            category,
            stage,
            if category == MigrationErrorCategory::DuplicateJsonKey {
                "JSON object contains a duplicate key"
            } else {
                "JSON could not be parsed"
            },
            None,
            None,
            None,
            source,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::Map;

    use super::*;

    static CALL_ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    // CALL_ORDER를 사용하는 테스트의 clear부터 마지막 assertion까지를 하나의 구간으로 묶는다.
    // callback은 별도의 CALL_ORDER mutex만 잠그므로 실행 중 재진입 deadlock이 생기지 않는다.
    static CALL_ORDER_TEST_GUARD: Mutex<()> = Mutex::new(());

    fn isolate_call_order_test() -> std::sync::MutexGuard<'static, ()> {
        CALL_ORDER_TEST_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_call_order() -> std::sync::MutexGuard<'static, Vec<&'static str>> {
        match CALL_ORDER.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // 이전 panic의 부분 기록은 호출자가 clear할 수 있도록 회수하고 poison 표시는 제거한다.
                let guard = poisoned.into_inner();
                CALL_ORDER.clear_poison();
                guard
            }
        }
    }

    fn clear_call_order() {
        lock_call_order().clear();
    }

    fn call_order_snapshot() -> Vec<&'static str> {
        lock_call_order().clone()
    }

    fn validate_object(value: &Value) -> Result<(), MigrationStepError> {
        if value.is_object() {
            Ok(())
        } else {
            Err(MigrationStepError::new("test value must be an object"))
        }
    }

    fn validate_has_old_name(value: &Value) -> Result<(), MigrationStepError> {
        if value.get("oldName").and_then(Value::as_str).is_some() {
            Ok(())
        } else {
            Err(MigrationStepError::new("test oldName is required"))
        }
    }

    fn validate_has_new_name(value: &Value) -> Result<(), MigrationStepError> {
        if value.get("newName").and_then(Value::as_str).is_some() {
            Ok(())
        } else {
            Err(MigrationStepError::new("test newName is required"))
        }
    }

    fn validate_has_details(value: &Value) -> Result<(), MigrationStepError> {
        if value
            .pointer("/details/name")
            .and_then(Value::as_str)
            .is_some()
        {
            Ok(())
        } else {
            Err(MigrationStepError::new("test details.name is required"))
        }
    }

    fn fail_validation(_value: &Value) -> Result<(), MigrationStepError> {
        Err(MigrationStepError::new("synthetic validation failure"))
    }

    fn migrate_1_to_2(mut value: Value) -> Result<Value, MigrationStepError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| MigrationStepError::new("test root must be an object"))?;
        let name = object
            .remove("oldName")
            .ok_or_else(|| MigrationStepError::new("test oldName is required"))?;
        object.insert("newName".to_owned(), name);
        object.insert(SCHEMA_VERSION_FIELD.to_owned(), Value::from(2));
        let trace = object
            .entry("trace")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| MigrationStepError::new("test trace must be an array"))?;
        trace.push(Value::String("1-2".to_owned()));
        Ok(value)
    }

    fn migrate_2_to_3(mut value: Value) -> Result<Value, MigrationStepError> {
        let object = value
            .as_object_mut()
            .ok_or_else(|| MigrationStepError::new("test root must be an object"))?;
        let name = object
            .remove("newName")
            .ok_or_else(|| MigrationStepError::new("test newName is required"))?;
        let details = object
            .entry("details")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| MigrationStepError::new("test details must be an object"))?;
        details.insert("name".to_owned(), name);
        object.insert(SCHEMA_VERSION_FIELD.to_owned(), Value::from(3));
        let trace = object
            .get_mut("trace")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| MigrationStepError::new("test trace must be an array"))?;
        trace.push(Value::String("2-3".to_owned()));
        Ok(value)
    }

    fn fail_step(_value: Value) -> Result<Value, MigrationStepError> {
        Err(MigrationStepError::new("synthetic step failure"))
    }

    fn wrong_output_version(mut value: Value) -> Result<Value, MigrationStepError> {
        value[SCHEMA_VERSION_FIELD] = Value::from(99);
        Ok(value)
    }

    fn change_root_type(_value: Value) -> Result<Value, MigrationStepError> {
        Ok(Value::Array(Vec::new()))
    }

    fn record_input(_value: &Value) -> Result<(), MigrationStepError> {
        lock_call_order().push("input");
        Ok(())
    }

    fn record_transform(mut value: Value) -> Result<Value, MigrationStepError> {
        lock_call_order().push("transform");
        value[SCHEMA_VERSION_FIELD] = Value::from(2);
        Ok(value)
    }

    fn record_output(_value: &Value) -> Result<(), MigrationStepError> {
        lock_call_order().push("output");
        Ok(())
    }

    fn record_failed_input(_value: &Value) -> Result<(), MigrationStepError> {
        lock_call_order().push("failed-input");
        Err(MigrationStepError::new("synthetic recorded failure"))
    }

    const STEP_1_TO_2: MigrationStep = MigrationStep::new(
        1,
        2,
        migrate_1_to_2,
        validate_has_old_name,
        validate_has_new_name,
    );
    const STEP_2_TO_3: MigrationStep = MigrationStep::new(
        2,
        3,
        migrate_2_to_3,
        validate_has_new_name,
        validate_has_details,
    );
    static TWO_STEPS: [MigrationStep; 2] = [STEP_1_TO_2, STEP_2_TO_3];

    fn registry_v3() -> MigrationRegistry<'static> {
        MigrationRegistry::try_new(3, &TWO_STEPS).unwrap()
    }

    fn required_plan<'a>(bytes: &[u8], registry: MigrationRegistry<'a>) -> MigrationPlan<'a> {
        match inspect_migration(bytes, registry).unwrap() {
            MigrationDecision::Required(plan) => plan,
            MigrationDecision::Current { .. } => panic!("test input should require migration"),
        }
    }

    #[test]
    fn production_registry_is_empty_and_derives_current_only_policy() {
        let registry = production_registry().unwrap();
        let policy = registry.support_policy().unwrap();
        assert_eq!(CURRENT_SCHEMA_VERSION.get(), 1);
        assert_eq!(registry.current(), CURRENT_SCHEMA_VERSION);
        assert_eq!(registry.minimum_migratable(), CURRENT_SCHEMA_VERSION);
        assert_eq!(policy.current(), CURRENT_SCHEMA_VERSION);
        assert_eq!(policy.minimum_migratable(), CURRENT_SCHEMA_VERSION);
        assert!(registry.steps.is_empty());
    }

    #[test]
    fn registry_accepts_single_and_multiple_ordered_steps() {
        let single = [STEP_1_TO_2];
        let registry = MigrationRegistry::try_new(2, &single).unwrap();
        assert_eq!(registry.minimum_migratable().get(), 1);
        assert_eq!(registry.current().get(), 2);
        let registry = registry_v3();
        assert_eq!(registry.minimum_migratable().get(), 1);
        assert_eq!(registry.steps.len(), 2);
    }

    #[test]
    fn registry_handles_u32_max_without_overflow_or_panic() {
        let empty = MigrationRegistry::try_new(u32::MAX, &[]).unwrap();
        assert_eq!(empty.current().get(), u32::MAX);
        assert_eq!(empty.minimum_migratable().get(), u32::MAX);

        let max_step = [MigrationStep::new(
            u32::MAX - 1,
            u32::MAX,
            fail_step,
            validate_object,
            validate_object,
        )];
        let registry = MigrationRegistry::try_new(u32::MAX, &max_step).unwrap();
        assert_eq!(registry.minimum_migratable().get(), u32::MAX - 1);

        let overflow = [MigrationStep::new(
            u32::MAX,
            0,
            fail_step,
            validate_object,
            validate_object,
        )];
        assert_eq!(
            MigrationRegistry::try_new(u32::MAX, &overflow)
                .unwrap_err()
                .category(),
            MigrationErrorCategory::InvalidRegistry
        );
    }

    #[test]
    fn registry_rejects_zero_gap_duplicate_order_reverse_excess_and_unreachable_steps() {
        let noop = validate_object;
        let transform = migrate_1_to_2;
        let cases = [
            (0, vec![]),
            (3, vec![MigrationStep::new(1, 3, transform, noop, noop)]),
            (3, vec![STEP_1_TO_2, STEP_1_TO_2]),
            (3, vec![STEP_2_TO_3, STEP_1_TO_2]),
            (2, vec![MigrationStep::new(2, 1, transform, noop, noop)]),
            (2, vec![MigrationStep::new(2, 3, transform, noop, noop)]),
            (
                4,
                vec![STEP_1_TO_2, MigrationStep::new(3, 4, transform, noop, noop)],
            ),
            (4, vec![STEP_1_TO_2, STEP_2_TO_3]),
        ];
        for (current, steps) in cases {
            assert_eq!(
                MigrationRegistry::try_new(current, &steps)
                    .unwrap_err()
                    .category(),
                MigrationErrorCategory::InvalidRegistry
            );
        }
    }

    #[test]
    fn current_is_returned_without_parsing_or_reserializing_full_json() {
        let input = b"{\"schemaVersion\":1,\"nested\":{\"same\":1,\"same\":2}}";
        match inspect_migration(input, production_registry().unwrap()).unwrap() {
            MigrationDecision::Current { version } => assert_eq!(version.get(), 1),
            MigrationDecision::Required(_) => panic!("production input should be current"),
        }
        assert_eq!(
            production_registry()
                .unwrap()
                .plan(CURRENT_SCHEMA_VERSION)
                .unwrap_err()
                .category(),
            MigrationErrorCategory::MigrationNotRequired
        );
    }

    #[test]
    fn migrates_one_step_and_two_steps_in_exact_order() {
        let one_steps = [STEP_1_TO_2];
        let one_registry = MigrationRegistry::try_new(2, &one_steps).unwrap();
        let one_input = r#"{"schemaVersion":1,"oldName":"세계"}"#.as_bytes();
        let one = required_plan(one_input, one_registry)
            .execute(one_input)
            .unwrap();
        assert_eq!(one.source_version.get(), 1);
        assert_eq!(one.final_version.get(), 2);
        assert_eq!(one.applied_steps.len(), 1);

        let input = r#"{"schemaVersion":1,"oldName":"세계","trace":[]}"#.as_bytes();
        let result = required_plan(input, registry_v3()).execute(input).unwrap();
        let value: Value = serde_json::from_slice(&result.bytes).unwrap();
        assert_eq!(value["trace"], serde_json::json!(["1-2", "2-3"]));
        assert_eq!(value["details"]["name"], "세계");
        assert_eq!(result.applied_steps.len(), 2);
    }

    #[test]
    fn preserves_unknown_fields_nested_objects_korean_and_array_order() {
        let input = r#"{"schemaVersion":1,"oldName":"한글 세계","unknownScalar":"그대로","unknown":{"깊이":{"더깊이":{"value":7}}},"details":{"preserved":true},"items":[{"한글키":"한글값"},null,true,42,1.25,{},[]],"emptyObject":{},"emptyArray":[],"trace":[]}"#
            .as_bytes();
        let result = required_plan(input, registry_v3()).execute(input).unwrap();
        let value: Value = serde_json::from_slice(&result.bytes).unwrap();
        assert_eq!(value["unknownScalar"], "그대로");
        assert_eq!(
            value.pointer("/unknown/깊이/더깊이/value"),
            Some(&Value::from(7))
        );
        assert_eq!(
            value["items"],
            serde_json::json!([{"한글키":"한글값"}, null, true, 42, 1.25, {}, []])
        );
        assert_eq!(value["emptyObject"], serde_json::json!({}));
        assert_eq!(value["emptyArray"], serde_json::json!([]));
        assert_eq!(value["details"]["name"], "한글 세계");
        assert_eq!(value["details"]["preserved"], true);
    }

    #[test]
    fn deterministic_output_has_required_encoding_and_layout() {
        let input =
            b"{ \"trace\": [], \"oldName\": \"world\", \"schemaVersion\": 1, \"z\": 1, \"a\": 2 }";
        let first = required_plan(input, registry_v3()).execute(input).unwrap();
        let second = required_plan(input, registry_v3()).execute(input).unwrap();
        assert_eq!(first.bytes, second.bytes);
        assert!(!first.bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert!(first.bytes.ends_with(b"\n"));
        assert!(!first.bytes.ends_with(b"\n\n"));
        let text = std::str::from_utf8(&first.bytes).unwrap();
        assert!(!text.contains("\r"));
        assert!(text.contains("\n  \"a\""));
        assert!(text.find("\"a\"").unwrap() < text.find("\"z\"").unwrap());
    }

    #[test]
    fn rejects_malformed_and_duplicate_keys_at_every_depth() {
        let malformed = br#"{"schemaVersion":1,"oldName":"x""#;
        assert_eq!(
            inspect_migration(malformed, registry_v3())
                .unwrap_err()
                .category(),
            MigrationErrorCategory::InvalidInputJson
        );
        for input in [
            br#"{"schemaVersion":1,"schemaVersion":1,"oldName":"x"}"#.as_slice(),
            br#"{"schemaVersion":1,"oldName":"x","nested":{"a":1,"a":2}}"#.as_slice(),
            br#"{"schemaVersion":1,"oldName":"x","items":[{"a":1,"a":2}]}"#.as_slice(),
            br#"{"schemaVersion":1,"oldName":"x","a":{"b":{"c":{"same":1,"same":2}}}}"#.as_slice(),
            br#"{"schemaVersion":1,"oldName":"x","name":1,"\u006eame":2}"#.as_slice(),
            r#"{"schemaVersion":1,"oldName":"x","한글":1,"한글":2}"#.as_bytes(),
        ] {
            let error = match inspect_migration(input, registry_v3()) {
                Ok(MigrationDecision::Required(plan)) => plan.execute(input).unwrap_err(),
                Err(error) => error,
                Ok(MigrationDecision::Current { .. }) => panic!("test input must not be current"),
            };
            assert_eq!(error.category(), MigrationErrorCategory::DuplicateJsonKey);
            let display = error.to_string();
            let debug = format!("{error:?}");
            for secret in ["same", "name", "한글", "oldName"] {
                assert!(!display.contains(secret));
                assert!(!debug.contains(secret));
            }
        }

        let repeated_in_separate_objects =
            br#"{"schemaVersion":1,"oldName":"x","left":{"same":1},"right":{"same":2}}"#;
        required_plan(repeated_in_separate_objects, registry_v3())
            .execute(repeated_in_separate_objects)
            .unwrap();
    }

    #[test]
    fn strict_parser_rejects_encoding_and_preserves_arbitrary_precision_numbers() {
        for input in [
            b"\xff".as_slice(),
            b"\xef\xbb\xbf{\"schemaVersion\":1}".as_slice(),
            b"{\"schemaVersion\":1} {\"other\":2}".as_slice(),
            b"{\"schemaVersion\":1} trailing".as_slice(),
            b"[]".as_slice(),
        ] {
            assert_eq!(
                inspect_migration(input, registry_v3())
                    .unwrap_err()
                    .category(),
                MigrationErrorCategory::InvalidInputJson
            );
        }

        let input = br#"{"schemaVersion":1,"oldName":"x","numbers":{"u64":18446744073709551615,"beyondU64":18446744073709551616,"long":1234567890123456789012345678901234567890,"decimal":0.123456789012345678901234567890,"exponent":1.234567890123456789e+100,"nested":[-0.000000000000000000000000000000000000000123456789]}}"#;
        let result = required_plan(input, registry_v3()).execute(input).unwrap();
        let text = std::str::from_utf8(&result.bytes).unwrap();
        for number in [
            "18446744073709551615",
            "18446744073709551616",
            "1234567890123456789012345678901234567890",
            "0.123456789012345678901234567890",
            "1.234567890123456789e+100",
            "-0.000000000000000000000000000000000000000123456789",
        ] {
            assert!(text.contains(number), "number lexeme changed: {number}");
        }
    }

    #[test]
    fn migration_rejects_reserved_marker_objects_without_output_or_input_mutation() {
        const RESERVED_KEYS: &[&str] = &[
            "$serde_json::private::Number",
            "$serde_json::private::RawValue",
            "$serde_json::private::FutureTransport",
        ];
        let inputs: &[&[u8]] = &[
            br#"{"schemaVersion":1,"oldName":"x","$serde_json::private::Number":"1"}"#,
            br#"{"schemaVersion":1,"oldName":"x","$serde_json::private::RawValue":"1"}"#,
            br#"{"schemaVersion":1,"oldName":"x","future":{"nested":{"$serde_json::private::Number":"1"}}}"#,
            br#"{"schemaVersion":1,"oldName":"x","future":[0,{"$serde_json::private::RawValue":"1"}]}"#,
            br#"{"schemaVersion":1,"oldName":"x","future":{"\u0024serde_json::private::\u004eumber":"1"}}"#,
            br#"{"schemaVersion":1,"oldName":"x","future":{"\u0024serde_json::private::\u0052awValue":"1"}}"#,
            br#"{"schemaVersion":1,"oldName":"x","future":{"$serde_json::private::FutureTransport":"1"}}"#,
        ];
        for input in inputs {
            let original = input.to_vec();
            let error = inspect_migration(input, registry_v3()).expect_err(
                "reserved object key must fail inspection before a MigrationPlan is returned",
            );
            assert_eq!(error.category(), MigrationErrorCategory::InvalidInputJson);
            assert_eq!(*input, original);

            let mut current: Option<&(dyn Error + 'static)> = Some(&error);
            while let Some(node) = current {
                for reserved_key in RESERVED_KEYS {
                    assert!(!node.to_string().contains(reserved_key));
                    assert!(!format!("{node:?}").contains(reserved_key));
                }
                current = node.source();
            }
        }

        let allowed = br#"{"schemaVersion":1,"oldName":"x","asString":"$serde_json::private::Number","future":{"$serde_json::private":"1","ordinaryFuture":"2"}}"#;
        let result = required_plan(allowed, registry_v3())
            .execute(allowed)
            .expect("marker string values and similar keys remain valid");
        let text = std::str::from_utf8(&result.bytes).unwrap();
        assert!(text.contains("$serde_json::private::Number"));
        assert!(text.contains("$serde_json::private"));
    }

    fn migration_input_with_array_depth(container_depth: usize) -> Vec<u8> {
        assert!(container_depth >= 1);
        let mut json = String::from(r#"{"schemaVersion":1,"oldName":"x","future":"#);
        for _ in 1..container_depth {
            json.push('[');
        }
        json.push('0');
        for _ in 1..container_depth {
            json.push(']');
        }
        json.push('}');
        json.into_bytes()
    }

    #[test]
    fn migration_rejects_depth_overflow_before_creating_a_plan_or_output() {
        let boundary = migration_input_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH);
        assert!(matches!(
            inspect_migration(&boundary, registry_v3())
                .expect("documented depth boundary should be inspectable"),
            MigrationDecision::Required(_)
        ));

        let too_deep =
            migration_input_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH + 1);
        let original = too_deep.clone();
        let error = inspect_migration(&too_deep, registry_v3())
            .expect_err("depth overflow must stop before a migration plan exists");
        assert_eq!(error.category(), MigrationErrorCategory::InvalidInputJson);
        assert_eq!(too_deep, original);
    }

    #[test]
    fn records_step_validation_and_transform_order_and_stops_on_failure() {
        let _call_order_test_guard = isolate_call_order_test();
        let input = br#"{"schemaVersion":1}"#;
        let step = [MigrationStep::new(
            1,
            2,
            record_transform,
            record_input,
            record_output,
        )];
        clear_call_order();
        required_plan(input, MigrationRegistry::try_new(2, &step).unwrap())
            .execute(input)
            .unwrap();
        let call_order = call_order_snapshot();
        assert_eq!(call_order, ["input", "transform", "output", "output"]);

        let failed = [MigrationStep::new(
            1,
            2,
            record_transform,
            record_failed_input,
            record_output,
        )];
        clear_call_order();
        required_plan(input, MigrationRegistry::try_new(2, &failed).unwrap())
            .execute(input)
            .unwrap_err();
        let call_order = call_order_snapshot();
        assert_eq!(call_order, ["failed-input"]);
    }

    #[test]
    fn blocks_future_and_unsupported_past_before_migration() {
        let _call_order_test_guard = isolate_call_order_test();
        clear_call_order();
        let future = inspect_migration(
            br#"{"schemaVersion":4,"nested":{"same":1,"same":2}}"#,
            registry_v3(),
        )
        .unwrap_err();
        assert_eq!(future.category(), MigrationErrorCategory::UnsupportedFuture);
        let steps = [STEP_2_TO_3];
        let registry = MigrationRegistry::try_new(3, &steps).unwrap();
        let past = inspect_migration(
            br#"{"schemaVersion":1,"nested":{"same":1,"same":2}}"#,
            registry,
        )
        .unwrap_err();
        assert_eq!(past.category(), MigrationErrorCategory::UnsupportedPast);
        let call_order_is_empty = call_order_snapshot().is_empty();
        assert!(call_order_is_empty);
    }

    #[test]
    fn recovers_poisoned_call_order_before_next_isolated_use() {
        let _call_order_test_guard = isolate_call_order_test();
        clear_call_order();
        lock_call_order().push("stale");

        let poisoning_thread = std::thread::spawn(|| {
            let _call_order = lock_call_order();
            panic!("synthetic CALL_ORDER poison");
        });
        assert!(poisoning_thread.join().is_err());
        assert!(CALL_ORDER.is_poisoned());

        clear_call_order();
        assert!(!CALL_ORDER.is_poisoned());
        record_input(&Value::Null).unwrap();
        let recovered_call_order = call_order_snapshot();
        clear_call_order();

        assert_eq!(recovered_call_order, ["input"]);
    }

    #[test]
    fn reports_step_and_validator_failures_without_returning_partial_output() {
        let failing_step = [MigrationStep::new(
            1,
            2,
            fail_step,
            validate_object,
            validate_object,
        )];
        let input = br#"{"schemaVersion":1,"oldName":"secret"}"#;
        let original = input.to_vec();
        let plan = required_plan(input, MigrationRegistry::try_new(2, &failing_step).unwrap());
        assert_eq!(
            plan.execute(input).unwrap_err().category(),
            MigrationErrorCategory::StepFailure
        );
        assert_eq!(input.as_slice(), original);

        let middle_failure = [
            STEP_1_TO_2,
            MigrationStep::new(2, 3, fail_step, validate_has_new_name, validate_object),
        ];
        let plan = required_plan(
            input,
            MigrationRegistry::try_new(3, &middle_failure).unwrap(),
        );
        assert_eq!(
            plan.execute(input).unwrap_err().category(),
            MigrationErrorCategory::StepFailure
        );
        assert_eq!(input.as_slice(), original);

        let bad_input_validator = [MigrationStep::new(
            1,
            2,
            migrate_1_to_2,
            fail_validation,
            validate_has_new_name,
        )];
        let plan = required_plan(
            input,
            MigrationRegistry::try_new(2, &bad_input_validator).unwrap(),
        );
        assert_eq!(
            plan.execute(input).unwrap_err().category(),
            MigrationErrorCategory::InputValidationFailure
        );

        let bad_output_validator = [MigrationStep::new(
            1,
            2,
            migrate_1_to_2,
            validate_has_old_name,
            fail_validation,
        )];
        let plan = required_plan(
            input,
            MigrationRegistry::try_new(2, &bad_output_validator).unwrap(),
        );
        assert_eq!(
            plan.execute(input).unwrap_err().category(),
            MigrationErrorCategory::OutputValidationFailure
        );
    }

    #[test]
    fn detects_input_output_and_final_version_mismatches() {
        let input_v1 = br#"{"schemaVersion":1,"oldName":"x"}"#;
        let input_v2 = br#"{"schemaVersion":2,"newName":"x"}"#;
        let registry = registry_v3();
        let plan = registry.plan(SchemaVersion::try_from(1).unwrap()).unwrap();
        assert_eq!(
            plan.execute(input_v2).unwrap_err().category(),
            MigrationErrorCategory::InputVersionMismatch
        );

        let wrong = [MigrationStep::new(
            1,
            2,
            wrong_output_version,
            validate_object,
            validate_object,
        )];
        let plan = required_plan(input_v1, MigrationRegistry::try_new(2, &wrong).unwrap());
        let error = plan.execute(input_v1).unwrap_err();
        assert_eq!(
            error.category(),
            MigrationErrorCategory::OutputVersionMismatch
        );
        assert_eq!(error.stage, MigrationStage::OutputValidation);
        assert_eq!(error.source_version.map(SchemaVersion::get), Some(99));
        assert_eq!(error.target_version.map(SchemaVersion::get), Some(2));
        assert_eq!(
            error.transition,
            Some(MigrationTransition {
                from: SchemaVersion::try_from(1).unwrap(),
                to: SchemaVersion::try_from(2).unwrap(),
            })
        );

        let wrong_root = [MigrationStep::new(
            1,
            2,
            change_root_type,
            validate_object,
            validate_object,
        )];
        let plan = required_plan(
            input_v1,
            MigrationRegistry::try_new(2, &wrong_root).unwrap(),
        );
        assert_eq!(
            plan.execute(input_v1).unwrap_err().category(),
            MigrationErrorCategory::OutputVersionMismatch
        );

        let short_steps = [STEP_1_TO_2];
        let invalid_plan = MigrationPlan {
            source: SchemaVersion::try_from(1).unwrap(),
            target: SchemaVersion::try_from(3).unwrap(),
            steps: &short_steps,
        };
        assert_eq!(
            invalid_plan.execute(input_v1).unwrap_err().category(),
            MigrationErrorCategory::FinalVersionMismatch
        );
    }

    #[test]
    fn migration_errors_do_not_expose_json_or_paths() {
        let input = br#"{"schemaVersion":1,"oldName":"private body","path":"C:\\Users\\name"}"#;
        let failing = [MigrationStep::new(
            1,
            2,
            fail_step,
            validate_object,
            validate_object,
        )];
        let display = required_plan(input, MigrationRegistry::try_new(2, &failing).unwrap())
            .execute(input)
            .unwrap_err()
            .to_string();
        assert!(!display.contains("private body"));
        assert!(!display.contains("C:\\Users"));
        let error = inspect_migration(br#"{"schemaVersion":"private field value"}"#, registry_v3())
            .unwrap_err();
        let debug = format!("{error:?}");
        assert!(!debug.contains("private field value"));
        assert!(error.source().is_some());
    }

    #[test]
    fn complete_parser_source_chains_are_redacted() {
        const CREDENTIAL: &str = "credential=migration-secret";
        const WINDOWS_PATH: &str = r"C:\Users\audit\private-project.json";
        const UNIX_PATH: &str = "/home/audit/private-project.json";
        const LABEL: &str = "private-field-label";
        const OPTION_LABEL: &str = "private-option-label";
        const DISCRIMINATOR: &str = "attacker-controlled-discriminator";
        const DUPLICATE_KEY: &str = "attacker-controlled-duplicate-key";
        let forbidden = [
            CREDENTIAL,
            "migration-secret",
            WINDOWS_PATH,
            UNIX_PATH,
            LABEL,
            OPTION_LABEL,
            DISCRIMINATOR,
            DUPLICATE_KEY,
        ];

        fn assert_chain(error: &(dyn Error + 'static), forbidden: &[&str]) -> usize {
            let mut count = 0;
            let mut current = Some(error);
            while let Some(node) = current {
                let display = node.to_string();
                let debug = format!("{node:?}");
                for secret in forbidden {
                    assert!(!display.contains(secret), "Display leaked {secret}");
                    assert!(!debug.contains(secret), "Debug leaked {secret}");
                }
                count += 1;
                current = node.source();
            }
            count
        }

        let plan = registry_v3()
            .plan(SchemaVersion::try_from(1).unwrap())
            .unwrap();
        let duplicate = format!(
            r#"{{"schemaVersion":1,"credential":"{CREDENTIAL}","windows":"C:\\Users\\audit\\private-project.json","unix":"{UNIX_PATH}","label":"{LABEL}","optionLabel":"{OPTION_LABEL}","kind":"{DISCRIMINATOR}","nested":{{"{DUPLICATE_KEY}":1,"{DUPLICATE_KEY}":2}}}}"#
        );
        let error = plan
            .execute(duplicate.as_bytes())
            .expect_err("nested duplicate must fail strict parsing");
        assert_eq!(error.category(), MigrationErrorCategory::DuplicateJsonKey);
        assert_eq!(assert_chain(&error, &forbidden), 3);

        let malformed = format!(
            r#"{{"schemaVersion":1,"credential":"{CREDENTIAL}","windows":"C:\\Users\\audit\\private-project.json","unix":"{UNIX_PATH}","label":"{LABEL}","optionLabel":"{OPTION_LABEL}","kind":"{DISCRIMINATOR}""#
        );
        let error = plan
            .execute(malformed.as_bytes())
            .expect_err("malformed JSON must fail strict parsing");
        assert_eq!(error.category(), MigrationErrorCategory::InvalidInputJson);
        assert_eq!(assert_chain(&error, &forbidden), 3);
    }
}
