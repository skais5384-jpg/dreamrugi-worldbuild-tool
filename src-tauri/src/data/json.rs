use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::{
    Impossible, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
};
use serde_json::{value::RawValue, Map, Value};

use crate::data::artifact::{ArtifactLosslessPath, ArtifactProvenanceAuthority};

const DUPLICATE_KEY_MARKER: &str = "STRICT_JSON_DUPLICATE_KEY";
const RESERVED_KEY_MARKER: &str = "STRICT_JSON_RESERVED_OBJECT_KEY";
const NESTING_DEPTH_MARKER: &str = "STRICT_JSON_NESTING_DEPTH_EXCEEDED";
const SERDE_JSON_NUMBER_TOKEN: &str = "$serde_json::private::Number";
const SERDE_JSON_RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";
const ROOT_JSON_CONTAINER_DEPTH: usize = 1;

/// 현재 JSON backend가 내부 transport에 쓰는 namespace 전체를 실제 object key에서 예약한다.
pub(crate) const SERDE_JSON_PRIVATE_NAMESPACE_PREFIX: &str = "$serde_json::private::";

/// serde_json 1.0.151은 128번째 container 진입을 거부한다. root object/array를 depth 1로 세므로
/// Worldbuild가 inspection과 materialization 양쪽에서 안정적으로 지원하는 최대값은 127이다.
pub(crate) const MAX_JSON_NESTING_DEPTH: usize = 127;

/// strict JSON 경계를 통과한 unknown 값의 숫자 표기를 잃지 않는 private 저장 표현이다.
///
/// 객체는 `BTreeMap`으로 보관해 encode 순서를 결정적으로 만들고, 배열 순서는 입력대로 유지한다.
/// 숫자만 원래 token을 보관하므로 입력의 공백이나 객체 insertion order까지 보존하지는 않는다.
#[derive(Clone, PartialEq)]
pub(crate) struct LosslessJsonValue(LosslessJsonKind);

/// Aggregate equality에는 영향을 주지 않는 codec 전용 원본 운반자다.
///
/// 값의 유무와 token 차이는 encode에서만 관찰하며 domain의 의미 비교에는 포함하지 않는다.
#[derive(Clone, Default)]
pub(crate) struct LosslessJsonSource(Option<Box<LosslessJsonValue>>);

impl LosslessJsonSource {
    /// strict decode를 마친 artifact만 최초 source를 부착할 수 있다.
    pub(crate) fn attach_artifact_source(
        &mut self,
        _authority: &ArtifactProvenanceAuthority,
        value: LosslessJsonValue,
    ) {
        self.0 = Some(Box::new(value));
    }

    fn value(&self) -> Option<&LosslessJsonValue> {
        self.0.as_deref()
    }

    pub(crate) fn artifact_value(
        &self,
        _authority: &ArtifactProvenanceAuthority,
    ) -> Option<&LosslessJsonValue> {
        self.value()
    }

    /// 검증을 마친 domain 변경 뒤 현재 wire를 새 기준으로 삼되, 같은 소유 위치의 숫자 원문만 옮긴다.
    ///
    /// 이 함수는 임의 JSON 편집용 API가 아니다. caller가 unknown storage 불변식을 먼저 검증한
    /// aggregate에만 사용해야 한다. 새 기준을 먼저 만들기 때문에 삭제된 subtree는 다시 생기지 않는다.
    pub(crate) fn rebase_artifact<T>(
        &mut self,
        _authority: &ArtifactProvenanceAuthority,
        value: &T,
    ) -> Result<(), JsonStorageError>
    where
        T: Serialize + ?Sized,
    {
        let mut current = materialize_validated_json_value(value)?.0;
        if let Some(source) = self.value() {
            restore_artifact_lexemes_owned(&mut current, source);
        }
        self.0 = Some(Box::new(current));
        Ok(())
    }

    /// artifact가 발급한 typed 위치에서만 subtree provenance를 복사한다.
    pub(crate) fn artifact_subtree(
        &self,
        _authority: &ArtifactProvenanceAuthority,
        path: ArtifactLosslessPath,
    ) -> Option<LosslessJsonValue> {
        let (field_id, member) = match path {
            ArtifactLosslessPath::TemplateCurrentDefault(field_id) => {
                (field_id.to_string(), "defaultValue")
            }
            ArtifactLosslessPath::TemplateInitialDefault(field_id) => {
                (field_id.to_string(), "initialDefaultValue")
            }
        };
        self.value()?
            .object_path(&["fields", field_id.as_str(), member])
            .cloned()
    }

    /// 다른 aggregate가 소유한 검증된 FieldValue를 typed Document 위치로만 전달한다.
    pub(crate) fn graft_artifact_field_value(
        &mut self,
        _authority: &ArtifactProvenanceAuthority,
        field_id: crate::data::artifact::FieldId,
        source: &LosslessJsonValue,
    ) -> bool {
        let Some(destination) = self.0.as_deref_mut() else {
            return false;
        };
        let field_id = field_id.to_string();
        let Some(destination) = destination.object_path_mut(&["fieldValues", field_id.as_str()])
        else {
            return false;
        };
        if !lossless_values_are_semantically_equal(destination, source) {
            return false;
        }
        *destination = source.clone();
        true
    }
}

impl PartialEq for LosslessJsonSource {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl fmt::Debug for LosslessJsonSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LosslessJsonSource")
            .field("present", &self.0.is_some())
            .field("payload_redacted", &true)
            .finish()
    }
}

#[derive(Clone, PartialEq)]
enum LosslessJsonKind {
    Null,
    Bool(bool),
    Number(Box<str>),
    String(String),
    Array(Vec<LosslessJsonValue>),
    Object(BTreeMap<String, LosslessJsonValue>),
}

impl fmt::Debug for LosslessJsonValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LosslessJsonValue")
            .field("kind", &self.kind_name())
            .field("payload_redacted", &true)
            .finish()
    }
}

impl LosslessJsonValue {
    fn kind_name(&self) -> &'static str {
        match self.0 {
            LosslessJsonKind::Null => "null",
            LosslessJsonKind::Bool(_) => "bool",
            LosslessJsonKind::Number(_) => "number",
            LosslessJsonKind::String(_) => "string",
            LosslessJsonKind::Array(_) => "array",
            LosslessJsonKind::Object(_) => "object",
        }
    }

    fn from_raw(raw: &RawValue) -> Result<Self, serde_json::Error> {
        match raw_token(raw) {
            Some(b'n') => {
                serde_json::from_str::<()>(raw.get())?;
                Ok(Self(LosslessJsonKind::Null))
            }
            Some(b't' | b'f') => serde_json::from_str::<bool>(raw.get())
                .map(|value| Self(LosslessJsonKind::Bool(value))),
            Some(b'"') => serde_json::from_str::<String>(raw.get())
                .map(|value| Self(LosslessJsonKind::String(value))),
            Some(b'[') => {
                let children = serde_json::from_str::<Vec<Box<RawValue>>>(raw.get())?;
                let values = children
                    .iter()
                    .map(|child| Self::from_raw(child))
                    .collect::<Result<_, _>>()?;
                Ok(Self(LosslessJsonKind::Array(values)))
            }
            Some(b'{') => {
                let children = serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(raw.get())?;
                let values = children
                    .into_iter()
                    .map(|(key, child)| Self::from_raw(&child).map(|value| (key, value)))
                    .collect::<Result<_, _>>()?;
                Ok(Self(LosslessJsonKind::Object(values)))
            }
            Some(_) => {
                // RawValue 파싱이 JSON 숫자 문법을 이미 확인했다. 그대로 보관한 token은
                // Serialize 때 RawValue로 다시 검증한 뒤에만 출력한다.
                serde_json::from_str::<Box<RawValue>>(raw.get())?;
                Ok(Self(LosslessJsonKind::Number(
                    raw.get().trim().to_owned().into_boxed_str(),
                )))
            }
            None => Err(<serde_json::Error as de::Error>::custom(
                "empty raw JSON value",
            )),
        }
    }

    pub(crate) fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(&serde_json::to_string(self)?)
    }

    pub(crate) fn object_path(&self, path: &[&str]) -> Option<&Self> {
        let mut current = self;
        for key in path {
            let LosslessJsonKind::Object(object) = &current.0 else {
                return None;
            };
            current = object.get(*key)?;
        }
        Some(current)
    }

    fn object_path_mut(&mut self, path: &[&str]) -> Option<&mut Self> {
        let mut current = self;
        for key in path {
            let LosslessJsonKind::Object(object) = &mut current.0 else {
                return None;
            };
            current = object.get_mut(*key)?;
        }
        Some(current)
    }
}
impl Serialize for LosslessJsonValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.0 {
            LosslessJsonKind::Null => serializer.serialize_none(),
            LosslessJsonKind::Bool(value) => serializer.serialize_bool(*value),
            LosslessJsonKind::Number(value) => RawValue::from_string(value.to_string())
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer),
            LosslessJsonKind::String(value) => serializer.serialize_str(value),
            LosslessJsonKind::Array(values) => values.serialize(serializer),
            LosslessJsonKind::Object(values) => values.serialize(serializer),
        }
    }
}

pub(crate) fn parse_strict_lossless_json_object(
    bytes: &[u8],
) -> Result<LosslessJsonValue, StrictJsonError> {
    let raw = parse_raw_json(bytes)?;
    if raw_token(&raw) != Some(b'{') {
        return Err(StrictJsonError::input(
            StrictJsonErrorCategory::RootTypeMismatch,
            "JSON root must be an object",
        ));
    }
    inspect_raw_value(&raw, RawInspectionMode::Strict, ROOT_JSON_CONTAINER_DEPTH)
        .map_err(strict_inspection_error)?;
    LosslessJsonValue::from_raw(&raw).map_err(|source| {
        StrictJsonError::json(
            StrictJsonErrorCategory::MalformedJson,
            "JSON could not be materialized losslessly",
            source,
        )
    })
}

pub(crate) fn validate_lossless_json_value_for_storage(
    value: &LosslessJsonValue,
) -> Result<(), JsonStorageError> {
    validate_lossless_json_value(value, 0)
}

fn validate_lossless_json_value(
    value: &LosslessJsonValue,
    parent_depth: usize,
) -> Result<(), JsonStorageError> {
    match &value.0 {
        LosslessJsonKind::Object(object) => {
            let depth = next_depth_or_error(parent_depth)?;
            if object.keys().any(|key| is_reserved_json_object_key(key)) {
                return Err(JsonStorageError::reserved_object_key());
            }
            for child in object.values() {
                validate_lossless_json_value(child, depth)?;
            }
            Ok(())
        }
        LosslessJsonKind::Array(items) => {
            let depth = next_depth_or_error(parent_depth)?;
            for item in items {
                validate_lossless_json_value(item, depth)?;
            }
            Ok(())
        }
        LosslessJsonKind::Null
        | LosslessJsonKind::Bool(_)
        | LosslessJsonKind::Number(_)
        | LosslessJsonKind::String(_) => Ok(()),
    }
}

pub(crate) fn is_reserved_json_object_key(key: &str) -> bool {
    key.starts_with(SERDE_JSON_PRIVATE_NAMESPACE_PREFIX)
}

fn next_json_container_depth(parent_depth: usize) -> Option<usize> {
    parent_depth
        .checked_add(1)
        .filter(|depth| *depth <= MAX_JSON_NESTING_DEPTH)
}

/// raw 입력과 encode 입력이 공유하는 저장 JSON 구조 오류 분류다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JsonStorageErrorCategory {
    NestingDepthExceeded,
    ReservedObjectKey,
    NonFiniteFloat,
    SerializationFailure,
}

/// payload를 보존하지 않는 고정 detail과 선택적인 원래 직렬화 오류를 분리한다.
pub(crate) struct JsonStorageError {
    category: JsonStorageErrorCategory,
    detail: &'static str,
    source: Option<serde_json::Error>,
}

impl JsonStorageError {
    pub(crate) const fn category(&self) -> JsonStorageErrorCategory {
        self.category
    }

    const fn fixed(category: JsonStorageErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            detail,
            source: None,
        }
    }

    fn nesting_depth() -> Self {
        Self::fixed(
            JsonStorageErrorCategory::NestingDepthExceeded,
            "JSON exceeds the supported nesting depth",
        )
    }

    fn reserved_object_key() -> Self {
        Self::fixed(
            JsonStorageErrorCategory::ReservedObjectKey,
            "JSON object contains a reserved interoperability key",
        )
    }

    fn non_finite_float() -> Self {
        Self::fixed(
            JsonStorageErrorCategory::NonFiniteFloat,
            "JSON does not support NaN or infinite numbers",
        )
    }

    fn serialization(source: serde_json::Error) -> Self {
        Self {
            category: JsonStorageErrorCategory::SerializationFailure,
            detail: "JSON value could not be serialized deterministically",
            source: Some(source),
        }
    }

    fn from_raw_inspection(source: serde_json::Error) -> Self {
        let message = source.to_string();
        if message.contains(NESTING_DEPTH_MARKER) {
            Self::nesting_depth()
        } else if message.contains(RESERVED_KEY_MARKER) {
            Self::reserved_object_key()
        } else {
            Self::serialization(source)
        }
    }

    fn into_serde_json_error(self) -> serde_json::Error {
        if let Some(source) = self.source {
            source
        } else {
            <serde_json::Error as serde::ser::Error>::custom(self.to_string())
        }
    }
}

impl serde::ser::Error for JsonStorageError {
    fn custom<T>(message: T) -> Self
    where
        T: fmt::Display,
    {
        Self::serialization(<serde_json::Error as serde::ser::Error>::custom(message))
    }
}

impl fmt::Debug for JsonStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonStorageError")
            .field("category", &self.category)
            .field("detail", &self.detail)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl fmt::Display for JsonStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "deterministic JSON validation failed ({:?}): {}",
            self.category, self.detail
        )
    }
}

impl Error for JsonStorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// 저장 artifact를 typed decode하기 전에 적용하는 공통 JSON 경계의 오류 분류다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StrictJsonErrorCategory {
    InvalidEncoding,
    MalformedJson,
    DuplicateKey,
    ReservedKey,
    NestingDepthExceeded,
    RootTypeMismatch,
}

/// 입력 본문 대신 안전한 분류와 고정 문구만 외부 진단에 노출한다.
pub(crate) struct StrictJsonError {
    category: StrictJsonErrorCategory,
    detail: &'static str,
    source: Option<serde_json::Error>,
}

impl StrictJsonError {
    pub(crate) const fn category(&self) -> StrictJsonErrorCategory {
        self.category
    }

    fn input(category: StrictJsonErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            detail,
            source: None,
        }
    }

    fn json(
        category: StrictJsonErrorCategory,
        detail: &'static str,
        source: serde_json::Error,
    ) -> Self {
        Self {
            category,
            detail,
            source: Some(source),
        }
    }
}

impl fmt::Debug for StrictJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StrictJsonError")
            .field("category", &self.category)
            .field("detail", &self.detail)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl fmt::Display for StrictJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "strict JSON inspection failed ({:?}): {}",
            self.category, self.detail
        )
    }
}

impl Error for StrictJsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// BOM 없는 UTF-8 JSON object를 읽고 모든 깊이의 중복 key와 예약 key를 거부한다.
///
/// `serde_json::Value`의 일반 역직렬화는 같은 key의 마지막 값을 남길 수 있으므로,
/// typed model보다 먼저 이 경계를 통과해야 보존할 수 없는 입력을 조용히 승인하지 않는다.
pub(crate) fn parse_strict_json_object(bytes: &[u8]) -> Result<Value, StrictJsonError> {
    let raw = parse_raw_json(bytes)?;
    if raw_token(&raw) != Some(b'{') {
        return Err(StrictJsonError::input(
            StrictJsonErrorCategory::RootTypeMismatch,
            "JSON root must be an object",
        ));
    }

    // 각 value를 RawValue로 먼저 받으면 실제 number token은 scalar로 남는다. 따라서
    // arbitrary_precision이 deserialize_any에 사용하는 synthetic map을 실제 object와
    // 구분하려고 해석하지 않고, 원본 token이 object/array일 때만 재귀 검사할 수 있다.
    inspect_raw_value(&raw, RawInspectionMode::Strict, ROOT_JSON_CONTAINER_DEPTH)
        .map_err(strict_inspection_error)?;

    let value: Value = serde_json::from_slice(bytes).map_err(|source| {
        StrictJsonError::json(
            StrictJsonErrorCategory::MalformedJson,
            "JSON could not be materialized without numeric precision loss",
            source,
        )
    })?;

    Ok(value)
}

/// 최소 compatibility reader도 unknown 본문을 materialize하지 않고 예약 key 정책만 공유한다.
pub(crate) fn inspect_reserved_json_object_keys(bytes: &[u8]) -> Result<(), StrictJsonError> {
    let raw = parse_raw_json(bytes)?;
    if raw_token(&raw) != Some(b'{') {
        return Err(StrictJsonError::input(
            StrictJsonErrorCategory::RootTypeMismatch,
            "JSON root must be an object",
        ));
    }
    inspect_raw_value(
        &raw,
        RawInspectionMode::ReservedOnly,
        ROOT_JSON_CONTAINER_DEPTH,
    )
    .map_err(strict_inspection_error)
}

fn parse_raw_json(bytes: &[u8]) -> Result<Box<RawValue>, StrictJsonError> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) || std::str::from_utf8(bytes).is_err() {
        return Err(StrictJsonError::input(
            StrictJsonErrorCategory::InvalidEncoding,
            "JSON must be valid UTF-8 without BOM",
        ));
    }
    serde_json::from_slice(bytes).map_err(|source| {
        StrictJsonError::json(
            StrictJsonErrorCategory::MalformedJson,
            "JSON could not be parsed as one complete value",
            source,
        )
    })
}

fn raw_token(raw: &RawValue) -> Option<u8> {
    raw.get().bytes().find(|byte| !byte.is_ascii_whitespace())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawInspectionMode {
    Strict,
    ReservedOnly,
}

fn inspect_raw_value(
    raw: &RawValue,
    mode: RawInspectionMode,
    depth: usize,
) -> Result<(), serde_json::Error> {
    if depth > MAX_JSON_NESTING_DEPTH {
        return Err(<serde_json::Error as de::Error>::custom(
            NESTING_DEPTH_MARKER,
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    match raw_token(raw) {
        Some(b'{') => RawObjectInspectionSeed { mode, depth }.deserialize(&mut deserializer)?,
        Some(b'[') => RawArrayInspectionSeed { mode, depth }.deserialize(&mut deserializer)?,
        _ => return Ok(()),
    }
    deserializer.end()
}

fn inspect_raw_child(
    raw: &RawValue,
    mode: RawInspectionMode,
    parent_depth: usize,
) -> Result<(), serde_json::Error> {
    if !matches!(raw_token(raw), Some(b'{' | b'[')) {
        return Ok(());
    }
    let child_depth = next_json_container_depth(parent_depth)
        .ok_or_else(|| <serde_json::Error as de::Error>::custom(NESTING_DEPTH_MARKER))?;
    inspect_raw_value(raw, mode, child_depth)
}

fn strict_inspection_error(source: serde_json::Error) -> StrictJsonError {
    let message = source.to_string();
    let (category, detail) = if message.contains(RESERVED_KEY_MARKER) {
        (
            StrictJsonErrorCategory::ReservedKey,
            "JSON object contains a reserved interoperability key",
        )
    } else if message.contains(NESTING_DEPTH_MARKER) {
        (
            StrictJsonErrorCategory::NestingDepthExceeded,
            "JSON exceeds the supported nesting depth",
        )
    } else if message.contains(DUPLICATE_KEY_MARKER) {
        (
            StrictJsonErrorCategory::DuplicateKey,
            "JSON object contains a duplicate key",
        )
    } else {
        (
            StrictJsonErrorCategory::MalformedJson,
            "JSON could not be inspected safely",
        )
    };
    StrictJsonError::json(category, detail, source)
}

fn propagate_inspection_error<E>(source: serde_json::Error) -> E
where
    E: de::Error,
{
    let message = source.to_string();
    if message.contains(RESERVED_KEY_MARKER) {
        E::custom(RESERVED_KEY_MARKER)
    } else if message.contains(NESTING_DEPTH_MARKER) {
        E::custom(NESTING_DEPTH_MARKER)
    } else if message.contains(DUPLICATE_KEY_MARKER) {
        E::custom(DUPLICATE_KEY_MARKER)
    } else {
        E::custom("STRICT_JSON_INSPECTION_FAILURE")
    }
}

struct RawObjectInspectionSeed {
    mode: RawInspectionMode,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for RawObjectInspectionSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(RawObjectInspectionVisitor {
            mode: self.mode,
            depth: self.depth,
        })
    }
}

struct RawObjectInspectionVisitor {
    mode: RawInspectionMode,
    depth: usize,
}

impl<'de> Visitor<'de> for RawObjectInspectionVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object with safe unique keys")
    }

    fn visit_map<A>(self, mut object: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if is_reserved_json_object_key(&key) {
                return Err(de::Error::custom(RESERVED_KEY_MARKER));
            }
            if self.mode == RawInspectionMode::Strict && !keys.insert(key) {
                return Err(de::Error::custom(DUPLICATE_KEY_MARKER));
            }
            let value = object.next_value::<Box<RawValue>>()?;
            inspect_raw_child(&value, self.mode, self.depth).map_err(propagate_inspection_error)?;
        }
        Ok(())
    }
}

struct RawArrayInspectionSeed {
    mode: RawInspectionMode,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for RawArrayInspectionSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(RawArrayInspectionVisitor {
            mode: self.mode,
            depth: self.depth,
        })
    }
}

struct RawArrayInspectionVisitor {
    mode: RawInspectionMode,
    depth: usize,
}

impl<'de> Visitor<'de> for RawArrayInspectionVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON array containing safe values")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(value) = sequence.next_element::<Box<RawValue>>()? {
            inspect_raw_child(&value, self.mode, self.depth).map_err(propagate_inspection_error)?;
        }
        Ok(())
    }
}

/// materialize된 JSON을 실제 wire container 규칙으로 최종 검증한다.
pub(crate) fn validate_json_value_for_storage(value: &Value) -> Result<(), JsonStorageError> {
    validate_json_value(value, 0)
}

/// rich-text처럼 object map 자체를 소유한 모델도 복사 없이 같은 정책을 적용한다.
pub(crate) fn validate_json_object_for_storage(
    object: &Map<String, Value>,
) -> Result<(), JsonStorageError> {
    validate_json_object(object, next_depth_or_error(0)?)
}

fn validate_json_value(value: &Value, parent_depth: usize) -> Result<(), JsonStorageError> {
    match value {
        Value::Object(object) => validate_json_object(object, next_depth_or_error(parent_depth)?),
        Value::Array(items) => {
            let depth = next_depth_or_error(parent_depth)?;
            for item in items {
                validate_json_value(item, depth)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_json_object(object: &Map<String, Value>, depth: usize) -> Result<(), JsonStorageError> {
    if object.keys().any(|key| is_reserved_json_object_key(key)) {
        return Err(JsonStorageError::reserved_object_key());
    }
    for child in object.values() {
        validate_json_value(child, depth)?;
    }
    Ok(())
}

fn next_depth_or_error(parent_depth: usize) -> Result<usize, JsonStorageError> {
    next_json_container_depth(parent_depth).ok_or_else(JsonStorageError::nesting_depth)
}

/// Serde 값으로부터 저장 규약을 따르는 결정적 JSON 문자열을 만든다.
///
/// 파일 쓰기는 담당하지 않으며, 호출자가 안전한 저장 절차를 적용할 수 있도록
/// 마지막 LF까지 포함된 완성된 내용만 반환한다.
pub fn to_deterministic_json_string<T>(value: &T) -> Result<String, serde_json::Error>
where
    T: Serialize + ?Sized,
{
    to_deterministic_json_string_categorized(value).map_err(JsonStorageError::into_serde_json_error)
}

/// 결정적 JSON을 BOM 없는 UTF-8 바이트로 반환한다.
pub fn to_deterministic_json_bytes<T>(value: &T) -> Result<Vec<u8>, serde_json::Error>
where
    T: Serialize + ?Sized,
{
    Ok(to_deterministic_json_string(value)?.into_bytes())
}

pub(crate) fn to_deterministic_json_bytes_categorized<T>(
    value: &T,
) -> Result<Vec<u8>, JsonStorageError>
where
    T: Serialize + ?Sized,
{
    Ok(to_deterministic_json_string_categorized(value)?.into_bytes())
}

/// 검증된 artifact가 소유한 source에서만 unknown number token을 복원한다.
///
/// current/source 의미 비교는 source 불일치를 막는 방어 조건일 뿐 provenance 증거가 아니다.
/// 호출 권한 자체가 decode 또는 닫힌 domain operation에서 유지한 ownership을 증명한다.
pub(crate) fn to_deterministic_artifact_json_bytes<T>(
    _authority: &ArtifactProvenanceAuthority,
    value: &T,
    source: &LosslessJsonValue,
) -> Result<Vec<u8>, JsonStorageError>
where
    T: Serialize + ?Sized,
{
    validate_lossless_json_value_for_storage(source)?;
    let mut canonical = materialize_validated_json_value(value)?;
    if lossless_values_are_semantically_equal(&canonical.0, source) {
        restore_artifact_lexemes_owned(&mut canonical.0, source);
    }
    let mut output =
        serde_json::to_string_pretty(&canonical).map_err(JsonStorageError::serialization)?;
    output.push('\n');
    Ok(output.into_bytes())
}

/// artifact ownership이 유지된 tree에만 적용한다. 이 함수 자체는 교체 이력을 추측하지 않는다.
/// artifact registry가 검증한 버전 전환은 schema 숫자만 바꾼다.
/// 나머지 tree는 재해석하지 않아 unknown 숫자의 원래 표기와 배열 순서를 보존한다.
pub(crate) fn artifact_schema_bytes(
    _authority: &ArtifactProvenanceAuthority,
    bytes: &[u8],
    version: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let mut tree = parse_strict_lossless_json_object(bytes)?;
    let LosslessJsonKind::Object(root) = &mut tree.0 else {
        unreachable!()
    };
    root.insert(
        "schemaVersion".into(),
        LosslessJsonValue(LosslessJsonKind::Number(
            version.to_string().into_boxed_str(),
        )),
    );
    let mut output = serde_json::to_vec_pretty(&tree)?;
    output.push(b'\n');
    Ok(output)
}

// 부제목은 배열 index가 아니라 Template 내부 ID가 unknown 정보의 owner다.
fn restore_artifact_lexemes_owned(current: &mut LosslessJsonValue, source: &LosslessJsonValue) {
    let mut aligned = source.clone();
    if let (LosslessJsonKind::Object(now), LosslessJsonKind::Object(old)) =
        (&current.0, &mut aligned.0)
    {
        if matches!(now.get("artifactType").map(|v|&v.0),Some(LosslessJsonKind::String(s)) if s=="template")
        {
            if let (
                Some(LosslessJsonValue(LosslessJsonKind::Array(items))),
                Some(LosslessJsonValue(LosslessJsonKind::Array(prior))),
            ) = (now.get("sections"), old.get_mut("sections"))
            {
                let id = |v: &LosslessJsonValue| match &v.0 {
                    LosslessJsonKind::Object(o) => o.get("id").cloned(),
                    _ => None,
                };
                *prior = items
                    .iter()
                    .map(|v| {
                        prior
                            .iter()
                            .find(|old| id(old) == id(v))
                            .unwrap_or(v)
                            .clone()
                    })
                    .collect();
            }
        }
    }
    restore_number_lexemes_owned(current, &aligned);
}

fn restore_number_lexemes_owned(current: &mut LosslessJsonValue, source: &LosslessJsonValue) {
    match (&mut current.0, &source.0) {
        (LosslessJsonKind::Number(current), LosslessJsonKind::Number(original)) => {
            if json_numbers_are_equal(current, original) {
                *current = original.clone();
            }
        }
        (LosslessJsonKind::Object(current), LosslessJsonKind::Object(original)) => {
            for (key, value) in current {
                if let Some(source_value) = original.get(key) {
                    restore_number_lexemes_owned(value, source_value);
                }
            }
        }
        (LosslessJsonKind::Array(current), LosslessJsonKind::Array(original))
            if current.len() == original.len() =>
        {
            for (value, source_value) in current.iter_mut().zip(original) {
                restore_number_lexemes_owned(value, source_value);
            }
        }
        _ => {}
    }
}

fn lossless_values_are_semantically_equal(
    left: &LosslessJsonValue,
    right: &LosslessJsonValue,
) -> bool {
    match (&left.0, &right.0) {
        (LosslessJsonKind::Null, LosslessJsonKind::Null) => true,
        (LosslessJsonKind::Bool(left), LosslessJsonKind::Bool(right)) => left == right,
        (LosslessJsonKind::Number(left), LosslessJsonKind::Number(right)) => {
            json_numbers_are_equal(left, right)
        }
        (LosslessJsonKind::String(left), LosslessJsonKind::String(right)) => left == right,
        (LosslessJsonKind::Array(left), LosslessJsonKind::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| lossless_values_are_semantically_equal(left, right))
        }
        (LosslessJsonKind::Object(left), LosslessJsonKind::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| lossless_values_are_semantically_equal(left, right))
                })
        }
        _ => false,
    }
}

fn json_numbers_are_equal(left: &str, right: &str) -> bool {
    match (normalized_number(left), normalized_number(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SignedDecimal {
    negative: bool,
    magnitude: String,
}

impl SignedDecimal {
    fn zero() -> Self {
        Self {
            negative: false,
            magnitude: "0".to_owned(),
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        let (negative, digits) = match raw.as_bytes().first() {
            Some(b'-') => (true, &raw[1..]),
            Some(b'+') => (false, &raw[1..]),
            Some(_) => (false, raw),
            None => return None,
        };
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        let magnitude = digits.trim_start_matches('0');
        if magnitude.is_empty() {
            return Some(Self::zero());
        }
        Some(Self {
            negative,
            magnitude: magnitude.to_owned(),
        })
    }

    fn add_unsigned(mut self, negative: bool, value: usize) -> Self {
        if value == 0 {
            return self;
        }
        let other = value.to_string();
        if self.magnitude == "0" {
            return Self {
                negative,
                magnitude: other,
            };
        }
        if self.negative == negative {
            self.magnitude = add_decimal_magnitudes(&self.magnitude, &other);
            return self;
        }
        match compare_decimal_magnitudes(&self.magnitude, &other) {
            std::cmp::Ordering::Greater => {
                self.magnitude = subtract_decimal_magnitudes(&self.magnitude, &other);
            }
            std::cmp::Ordering::Less => {
                self.magnitude = subtract_decimal_magnitudes(&other, &self.magnitude);
                self.negative = negative;
            }
            std::cmp::Ordering::Equal => return Self::zero(),
        }
        self
    }
}

fn compare_decimal_magnitudes(left: &str, right: &str) -> std::cmp::Ordering {
    left.len()
        .cmp(&right.len())
        .then_with(|| left.as_bytes().cmp(right.as_bytes()))
}

fn add_decimal_magnitudes(left: &str, right: &str) -> String {
    let mut left = left.bytes().rev();
    let mut right = right.bytes().rev();
    let mut carry = 0_u8;
    let mut reversed = Vec::with_capacity(left.len().max(right.len()) + 1);
    loop {
        let left = left.next().map(|digit| digit - b'0');
        let right = right.next().map(|digit| digit - b'0');
        if left.is_none() && right.is_none() && carry == 0 {
            break;
        }
        let sum = left.unwrap_or(0) + right.unwrap_or(0) + carry;
        reversed.push(b'0' + sum % 10);
        carry = sum / 10;
    }
    reversed.reverse();
    reversed.into_iter().map(char::from).collect()
}

/// left >= right인 canonical decimal magnitude의 차를 계산한다.
fn subtract_decimal_magnitudes(left: &str, right: &str) -> String {
    let mut right = right.bytes().rev();
    let mut borrow = 0_i16;
    let mut reversed = Vec::with_capacity(left.len());
    for left_digit in left.bytes().rev() {
        let mut digit = i16::from(left_digit - b'0') - borrow;
        let right_digit = i16::from(right.next().map_or(0, |digit| digit - b'0'));
        if digit < right_digit {
            digit += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        let difference = digit - right_digit;
        debug_assert!((0..=9).contains(&difference));
        reversed.push(b'0' + difference as u8);
    }
    while reversed.len() > 1 && reversed.last() == Some(&b'0') {
        reversed.pop();
    }
    reversed.reverse();
    reversed.into_iter().map(char::from).collect()
}

fn normalized_number(raw: &str) -> Option<(bool, String, SignedDecimal)> {
    let unsigned = raw.strip_prefix('-').unwrap_or(raw);
    let negative = raw.starts_with('-');
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, SignedDecimal::parse(exponent)?),
        None => (unsigned, SignedDecimal::zero()),
    };
    let fractional_digits = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let digits = mantissa.replace('.', "");
    let significant = digits.trim_start_matches('0');
    if significant.is_empty() {
        return Some((false, "0".to_owned(), SignedDecimal::zero()));
    }
    let trimmed = significant.trim_end_matches('0');
    let trailing_zeros = significant.len().checked_sub(trimmed.len())?;
    let scale = exponent
        .add_unsigned(true, fractional_digits)
        .add_unsigned(false, trailing_zeros);
    Some((negative, trimmed.to_owned(), scale))
}

/// Serialize 대상 전체를 실제 wire root 기준으로 materialize해 저장 가능성만 확인한다.
///
/// byte를 만들지 않는 domain admission도 encoder와 같은 depth/reserved namespace 규칙을
/// 사용해야 한다. 반환할 JSON을 노출하지 않아 caller가 이 함수를 우회 편집 경로로 쓰지 못하게 한다.
pub(crate) fn validate_serializable_for_storage<T>(value: &T) -> Result<(), JsonStorageError>
where
    T: Serialize + ?Sized,
{
    materialize_validated_json_value(value).map(drop)
}

fn to_deterministic_json_string_categorized<T>(value: &T) -> Result<String, JsonStorageError>
where
    T: Serialize + ?Sized,
{
    let sorted = materialize_validated_json_value(value)?;

    let mut output =
        serde_json::to_string_pretty(&sorted).map_err(JsonStorageError::serialization)?;
    output.push('\n');
    Ok(output)
}

fn materialize_validated_json_value<T>(value: &T) -> Result<ValidatedJsonValue, JsonStorageError>
where
    T: Serialize + ?Sized,
{
    // materialization 전에 모든 JSON container 경로를 bounded traversal로 먼저 돈다.
    // 프로젝트의 derive 기반 Serialize 값은 외부 상태를 읽지 않으므로 두 번 방문해도 같다.
    value.serialize(JsonSerializeValidator::root())?;

    let serialized = serde_json::to_vec(value).map_err(JsonStorageError::serialization)?;
    let raw = serde_json::from_slice::<Box<RawValue>>(&serialized)
        .map_err(JsonStorageError::serialization)?;
    let json_value = LosslessJsonValue::from_raw(&raw).map_err(JsonStorageError::serialization)?;
    validate_lossless_json_value_for_storage(&json_value)?;
    // 기존 Value 기반 검증과의 동치도 유지하되, 이 값은 검증에만 쓰고 출력에는 사용하지 않는다.
    let semantic_value = json_value
        .to_value()
        .map_err(JsonStorageError::serialization)?;
    validate_json_value_for_storage(&semantic_value)?;
    Ok(ValidatedJsonValue(json_value))
}

/// 검증 성공을 타입으로 표시해 정렬이 검증보다 먼저 호출되지 않게 한다.
struct ValidatedJsonValue(LosslessJsonValue);

impl Serialize for ValidatedJsonValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

#[derive(Debug, Clone, Copy)]
struct JsonSerializeValidator {
    container_depth: usize,
}

impl JsonSerializeValidator {
    const fn root() -> Self {
        Self { container_depth: 0 }
    }

    fn enter_container(self) -> Result<Self, JsonStorageError> {
        Ok(Self {
            container_depth: next_depth_or_error(self.container_depth)?,
        })
    }

    fn enter_two_containers(self) -> Result<Self, JsonStorageError> {
        self.enter_container()?.enter_container()
    }

    fn validate_float(value: f64) -> Result<(), JsonStorageError> {
        if value.is_finite() {
            Ok(())
        } else {
            Err(JsonStorageError::non_finite_float())
        }
    }
}

impl Serializer for JsonSerializeValidator {
    type Ok = ();
    type Error = JsonStorageError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = JsonSerializeStructValidator;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        Self::validate_float(value.into())
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        Self::validate_float(value)
    }

    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_str(self, _value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.enter_container().map(|_| ())
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(self.enter_container()?)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.enter_container()
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.enter_container()
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.enter_container()
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.enter_two_containers()
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.enter_container()
    }
    fn serialize_struct(
        self,
        name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        match name {
            SERDE_JSON_NUMBER_TOKEN => Ok(JsonSerializeStructValidator::NumberTransport(self)),
            SERDE_JSON_RAW_VALUE_TOKEN => Ok(JsonSerializeStructValidator::RawValueTransport {
                parent_depth: self.container_depth,
            }),
            _ => Ok(JsonSerializeStructValidator::Object(
                self.enter_container()?,
            )),
        }
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.enter_two_containers()
    }
}

macro_rules! validate_element {
    ($trait_name:ident, $method:ident) => {
        impl $trait_name for JsonSerializeValidator {
            type Ok = ();
            type Error = JsonStorageError;

            fn $method<T>(&mut self, value: &T) -> Result<(), Self::Error>
            where
                T: Serialize + ?Sized,
            {
                value.serialize(*self)
            }

            fn end(self) -> Result<Self::Ok, Self::Error> {
                Ok(())
            }
        }
    };
}

validate_element!(SerializeSeq, serialize_element);
validate_element!(SerializeTuple, serialize_element);
validate_element!(SerializeTupleStruct, serialize_field);
validate_element!(SerializeTupleVariant, serialize_field);

impl SerializeMap for JsonSerializeValidator {
    type Ok = ();
    type Error = JsonStorageError;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: Serialize + ?Sized,
    {
        key.serialize(*self)
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize + ?Sized,
    {
        value.serialize(*self)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

macro_rules! validate_struct_field {
    ($trait_name:ident) => {
        impl $trait_name for JsonSerializeValidator {
            type Ok = ();
            type Error = JsonStorageError;

            fn serialize_field<T>(
                &mut self,
                _key: &'static str,
                value: &T,
            ) -> Result<(), Self::Error>
            where
                T: Serialize + ?Sized,
            {
                value.serialize(*self)
            }

            fn end(self) -> Result<Self::Ok, Self::Error> {
                Ok(())
            }
        }
    };
}

validate_struct_field!(SerializeStructVariant);

enum JsonSerializeStructValidator {
    Object(JsonSerializeValidator),
    NumberTransport(JsonSerializeValidator),
    RawValueTransport { parent_depth: usize },
}

impl SerializeStruct for JsonSerializeStructValidator {
    type Ok = ();
    type Error = JsonStorageError;

    fn serialize_field<T>(&mut self, _key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: Serialize + ?Sized,
    {
        match self {
            Self::Object(validator) | Self::NumberTransport(validator) => {
                value.serialize(*validator)
            }
            Self::RawValueTransport { parent_depth } => value.serialize(RawJsonStringValidator {
                parent_depth: *parent_depth,
            }),
        }
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(())
    }
}

struct RawJsonStringValidator {
    parent_depth: usize,
}

impl RawJsonStringValidator {
    fn invalid_transport() -> JsonStorageError {
        JsonStorageError::serialization(<serde_json::Error as serde::ser::Error>::custom(
            "invalid serde_json raw-value transport",
        ))
    }
}

impl Serializer for RawJsonStringValidator {
    type Ok = ();
    type Error = JsonStorageError;
    type SerializeSeq = Impossible<(), JsonStorageError>;
    type SerializeTuple = Impossible<(), JsonStorageError>;
    type SerializeTupleStruct = Impossible<(), JsonStorageError>;
    type SerializeTupleVariant = Impossible<(), JsonStorageError>;
    type SerializeMap = Impossible<(), JsonStorageError>;
    type SerializeStruct = Impossible<(), JsonStorageError>;
    type SerializeStructVariant = Impossible<(), JsonStorageError>;

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        let raw: Box<RawValue> =
            serde_json::from_str(value).map_err(JsonStorageError::serialization)?;
        if matches!(raw_token(&raw), Some(b'{' | b'[')) {
            let depth = next_depth_or_error(self.parent_depth)?;
            inspect_raw_value(&raw, RawInspectionMode::Strict, depth)
                .map_err(JsonStorageError::from_raw_inspection)?;
        }
        Ok(())
    }

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_some<T>(self, _value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        Err(Self::invalid_transport())
    }
    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_newtype_struct<T>(
        self,
        _name: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        Err(Self::invalid_transport())
    }
    fn serialize_newtype_variant<T>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: Serialize + ?Sized,
    {
        Err(Self::invalid_transport())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Err(Self::invalid_transport())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(Self::invalid_transport())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use serde::{ser::Error as _, Serialize};
    use serde_json::json;

    use super::*;

    #[test]
    fn maps_with_different_input_order_have_identical_output() {
        let first = HashMap::from([("z", 1), ("a", 2)]);
        let second = HashMap::from([("a", 2), ("z", 1)]);

        assert_eq!(serialize(&first), serialize(&second));
    }

    #[test]
    fn nested_object_keys_are_sorted_and_array_order_is_preserved() {
        let value = json!({"z": {"y": {"z": 1, "a": 2}, "a": 3}, "a": [3, 1, 2]});
        let output = serialize(&value);

        assert_eq!(output, "{\n  \"a\": [\n    3,\n    1,\n    2\n  ],\n  \"z\": {\n    \"a\": 3,\n    \"y\": {\n      \"a\": 2,\n      \"z\": 1\n    }\n  }\n}\n");
    }

    #[test]
    fn output_has_lf_two_space_indent_one_final_newline_and_no_bom() {
        let output = serialize(&json!({"outer": {"value": true}}));

        assert_eq!(output, "{\n  \"outer\": {\n    \"value\": true\n  }\n}\n");
        assert!(!output.contains("\r\n"));
        assert!(!output.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]));
        assert!(output.ends_with('\n'));
        assert!(!output.ends_with("\n\n"));
    }

    #[test]
    fn korean_text_round_trips_without_escaping_or_damage() {
        let output = serialize(&json!({"이름": "세계관"}));
        let decoded: Value = serde_json::from_str(&output).expect("generated JSON should parse");

        assert!(output.contains("세계관"));
        assert_eq!(decoded["이름"], "세계관");
    }

    #[test]
    fn rejects_each_non_finite_float() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = to_deterministic_json_bytes_categorized(&value)
                .expect_err("non-finite values must fail before materialization");
            assert_eq!(error.category(), JsonStorageErrorCategory::NonFiniteFloat);
            assert!(error.source().is_none());
        }

        assert!(to_deterministic_json_string(&f32::NAN).is_err());
        for input in [
            br#"{"value":NaN}"#.as_slice(),
            br#"{"value":Infinity}"#,
            br#"{"value":-Infinity}"#,
        ] {
            assert_eq!(
                parse_strict_json_object(input).unwrap_err().category(),
                StrictJsonErrorCategory::MalformedJson
            );
        }
    }

    #[test]
    fn strict_parse_and_deterministic_serialize_preserve_number_lexemes() {
        let raw = br#"{"z":[0.123456789012345678901234567890,{"integer":1234567890123456789012345678901234567890}],"a":{"exponent":1.234567890123456789e+100,"small":-0.000000000000000000000000000000000000000123456789}}"#;
        let value = parse_strict_json_object(raw).expect("arbitrary precision JSON should parse");
        let first = to_deterministic_json_bytes(&value).expect("number should serialize");
        let text = std::str::from_utf8(&first).expect("deterministic JSON is UTF-8");
        for number in [
            "0.123456789012345678901234567890",
            "1234567890123456789012345678901234567890",
            "1.234567890123456789e+100",
            "-0.000000000000000000000000000000000000000123456789",
        ] {
            assert!(text.contains(number), "number lexeme changed: {number}");
        }
        let reparsed = parse_strict_json_object(&first).expect("output should strict-parse");
        let second = to_deterministic_json_bytes(&reparsed).expect("number should reserialize");
        assert_eq!(second, first);
    }

    #[test]
    fn numeric_equivalence_is_not_treated_as_provenance() {
        for (left, right) in [("1E100", "1e100"), ("1e+2", "100"), ("-0", "0")] {
            assert!(json_numbers_are_equal(left, right));
        }

        // 일반 serializer에는 source를 전달할 수 없으므로 같은 값이어도 current 표기만 출력한다.
        let current: Value = serde_json::from_str(r#"{"a":1e100,"array":[100],"negativeZero":0}"#)
            .expect("current must decode");
        let encoded = to_deterministic_json_bytes(&current).expect("current must encode");
        let text = std::str::from_utf8(&encoded).expect("output is UTF-8");
        for stale in ["1E100", "1e+2", "-0"] {
            assert!(!text.contains(stale));
        }
    }

    #[test]
    fn arbitrary_exponent_lexemes_round_trip_without_fixed_integer_limits() {
        let tokens = [
            "1E9223372036854775807",
            "1E9223372036854775808",
            "1E-9223372036854775808",
            "1E-9223372036854775809",
            "1E0009223372036854775808",
            "1e+0009223372036854775808",
            "123456789012345678901234567890.1234567890123456789000E+999999999999999999999",
            "1000000000000000000000000000000e-999999999999999999999",
            "0",
            "-0",
            "0e999999999999999999999",
            "-0E-999999999999999999999",
        ];
        for token in tokens {
            let raw = format!(r#"{{"number":{token}}}"#);
            let source = parse_strict_lossless_json_object(raw.as_bytes())
                .expect("valid arbitrary exponent must pass strict parsing");
            let current: Value = serde_json::from_str(&raw).expect("number must materialize");
            let current = materialize_validated_json_value(&current)
                .expect("number must remain storage-valid");
            assert!(
                lossless_values_are_semantically_equal(&current.0, &source),
                "number comparison changed: {token}"
            );
        }

        for (source_token, current_token) in [
            ("1E9223372036854775808", "2e+9223372036854775808"),
            ("1E-9223372036854775809", "1.1e-9223372036854775809"),
            (
                "1000000000000000000000000000000e-999999999999999999999",
                "1000000000000000000000000000001e-999999999999999999999",
            ),
        ] {
            assert!(
                !json_numbers_are_equal(source_token, current_token),
                "different arbitrary exponents must not compare equal"
            );
        }

        let equivalent = [
            ("1e100", "10e99"),
            ("10e99", "100e98"),
            ("1.2300e10", "123e8"),
            ("0e999999999999999999999", "-0E-999999999999999999999"),
        ];
        for (left, right) in equivalent {
            assert!(json_numbers_are_equal(left, right), "{left} != {right}");
        }
    }

    #[test]
    fn equal_value_replacements_without_artifact_provenance_are_canonical() {
        let cases = [
            (r#"{"a":[1E100,1e100]}"#, r#"{"a":[1e100,1E100]}"#),
            (
                r#"{"a":[{"n":1E100},{"n":1e100}]}"#,
                r#"{"a":[{"n":1e100},{"n":1E100}]}"#,
            ),
            (r#"{"a":[-0E100,0]}"#, r#"{"a":[0,-0E100]}"#),
            (
                r#"{"a":{"items":[1E100,1e100]}}"#,
                r#"{"a":{"items":[1e100,1E100]}}"#,
            ),
            (r#"{"a":{"n":1E100}}"#, r#"{"a":{"n":1e100}}"#),
            (
                r#"{"a":[{"kind":"old","n":1E100}]}"#,
                r#"{"a":[{"kind":"new","n":1e100}]}"#,
            ),
            (r#"{"a":[1E100]}"#, r#"{"a":[1e100,2]}"#),
            (r#"{"a":[1E100,2]}"#, r#"{"a":[1e100]}"#),
            (
                r#"{"a":{"items":[{"kind":"old","n":1E100}]}}"#,
                r#"{"a":{"items":[{"kind":"new","n":1e100}]}}"#,
            ),
            (r#"{"a":{"from":1E100}}"#, r#"{"a":{"to":1e100}}"#),
        ];

        for (source, current) in cases {
            parse_strict_lossless_json_object(source.as_bytes())
                .expect("source must pass strict admission");
            let current: Value = serde_json::from_str(current).expect("current must decode");
            let encoded = to_deterministic_json_bytes(&current)
                .expect("provenance-free current must encode canonically");
            let text = std::str::from_utf8(&encoded).expect("output is UTF-8");
            assert!(!text.contains("1E100"));
            assert!(!text.contains("-0E100"));
            assert!(!text.contains("\"from\""));
            parse_strict_json_object(&encoded).expect("canonical output must strict-decode");
        }
    }

    #[test]
    fn raw_inspection_rejects_the_private_namespace_only_for_object_keys() {
        const NUMBER_MARKER: &str = "$serde_json::private::Number";
        const RAW_VALUE_MARKER: &str = "$serde_json::private::RawValue";
        let invalid: &[&[u8]] = &[
            br#"{"$serde_json::private::Number":"1"}"#,
            br#"{"$serde_json::private::RawValue":"1"}"#,
            br#"{"future":{"$serde_json::private::Number":"1"}}"#,
            br#"{"future":{"nested":{"$serde_json::private::RawValue":"1"}}}"#,
            br#"{"future":[0,{"$serde_json::private::RawValue":"1"}]}"#,
            br#"{"future":{"\u0024serde_json::private::\u004eumber":"1"}}"#,
            br#"{"future":{"\u0024serde_json::private::\u0052awValue":"1"}}"#,
            br#"{"future":{"$serde_json::private::FutureTransport":"1"}}"#,
        ];
        for input in invalid {
            let error = parse_strict_json_object(input).expect_err("reserved key must fail");
            assert_eq!(error.category(), StrictJsonErrorCategory::ReservedKey);
            let mut current: Option<&(dyn Error + 'static)> = Some(&error);
            while let Some(node) = current {
                for marker in [NUMBER_MARKER, RAW_VALUE_MARKER] {
                    assert!(!node.to_string().contains(marker));
                    assert!(!format!("{node:?}").contains(marker));
                }
                current = node.source();
            }
        }

        let allowed = format!(
            r#"{{"asString":"{}","future":{{"$serde_json::private":"1","prefix$serde_json::private::Number":"2","ordinaryFuture":"3"}},"number":1234567890123456789012345678901234567890}}"#,
            NUMBER_MARKER
        );
        let parsed = parse_strict_json_object(allowed.as_bytes())
            .expect("string values, similar keys, and real numbers remain valid");
        assert_eq!(parsed["asString"], Value::String(NUMBER_MARKER.to_owned()));

        let marker_object = Value::Object(Map::from_iter([(
            "$serde_json::private::FutureTransport".to_owned(),
            Value::String("1".to_owned()),
        )]));
        let value = Value::Object(Map::from_iter([("future".to_owned(), marker_object)]));
        let error = to_deterministic_json_bytes_categorized(&value)
            .expect_err("encode must reject a programmatically assembled reserved object key");
        assert_eq!(
            error.category(),
            JsonStorageErrorCategory::ReservedObjectKey
        );
        assert!(!error.to_string().contains("FutureTransport"));
    }

    #[test]
    fn pinned_serde_json_transport_behavior_matches_the_strict_boundary_assumption() {
        const NUMBER_MARKER: &str = "$serde_json::private::Number";
        const RAW_VALUE_MARKER: &str = "$serde_json::private::RawValue";
        let number_raw = r#"{"$serde_json::private::Number":"123456789012345678901234567890"}"#;
        let raw_value_raw = r#"{"$serde_json::private::RawValue":"true"}"#;

        // 이 직접 decode는 고정한 backend의 private transport 충돌을 관찰하는 update gate일 뿐,
        // production 입력 경로가 아니다. 동작이 바뀌면 namespace 방어를 다시 감사해야 한다.
        let number: Value = serde_json::from_str(number_raw)
            .expect("pinned backend should recognize its number transport marker");
        assert!(number.is_number());
        assert_eq!(number.to_string(), "123456789012345678901234567890");
        let raw_value: Value = serde_json::from_str(raw_value_raw)
            .expect("pinned backend should recognize its raw-value transport marker");
        assert_eq!(raw_value, Value::Bool(true));

        for (marker, raw) in [
            (NUMBER_MARKER, number_raw),
            (RAW_VALUE_MARKER, raw_value_raw),
        ] {
            assert!(is_reserved_json_object_key(marker));
            assert_eq!(
                parse_strict_json_object(raw.as_bytes())
                    .expect_err("strict inspection must run before Value materialization")
                    .category(),
                StrictJsonErrorCategory::ReservedKey
            );
        }
    }

    fn nested_object(container_depth: usize) -> Vec<u8> {
        assert!(container_depth >= 1);
        let mut json = String::new();
        for _ in 0..container_depth {
            json.push_str(r#"{"value":"#);
        }
        json.push('0');
        for _ in 0..container_depth {
            json.push('}');
        }
        json.into_bytes()
    }

    fn nested_array(container_depth: usize) -> Vec<u8> {
        assert!(container_depth >= 1);
        let mut json = String::from(r#"{"value":"#);
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

    fn alternating_containers(container_depth: usize) -> Vec<u8> {
        assert!(container_depth >= 1);
        let mut json = String::from(r#"{"value":"#);
        let mut closers = Vec::new();
        for depth in 2..=container_depth {
            if depth % 2 == 0 {
                json.push('[');
                closers.push(']');
            } else {
                json.push_str(r#"{"value":"#);
                closers.push('}');
            }
        }
        json.push('0');
        for closer in closers.into_iter().rev() {
            json.push(closer);
        }
        json.push('}');
        json.into_bytes()
    }

    #[test]
    fn global_nesting_budget_has_stable_object_array_and_mixed_boundaries() {
        for fixture in [nested_object, nested_array, alternating_containers] {
            parse_strict_json_object(&fixture(MAX_JSON_NESTING_DEPTH))
                .expect("the documented container boundary should be supported");
            let error = parse_strict_json_object(&fixture(MAX_JSON_NESTING_DEPTH + 1))
                .expect_err("one container beyond the boundary must fail safely");
            assert_eq!(
                error.category(),
                StrictJsonErrorCategory::NestingDepthExceeded
            );
        }
    }

    #[derive(Serialize)]
    struct RawValueEnvelope<'a> {
        raw: &'a RawValue,
    }

    fn assert_raw_envelope_success(
        label: &str,
        raw: &RawValue,
        expected_raw_value: &Value,
        expected_wire_depth: usize,
    ) -> Vec<u8> {
        raw.serialize(JsonSerializeValidator::root())
            .unwrap_or_else(|error| panic!("{label}: direct raw preflight failed: {error}"));
        assert_eq!(
            serde_json::to_value(raw)
                .unwrap_or_else(|error| panic!("{label}: raw materialization failed: {error}")),
            *expected_raw_value,
            "{label}: RawValue semantics changed during materialization"
        );

        let envelope = RawValueEnvelope { raw };
        envelope
            .serialize(JsonSerializeValidator::root())
            .unwrap_or_else(|error| panic!("{label}: envelope preflight failed: {error}"));
        let materialized = serde_json::to_value(&envelope)
            .unwrap_or_else(|error| panic!("{label}: envelope materialization failed: {error}"));
        assert_eq!(materialized["raw"], *expected_raw_value);
        assert_eq!(
            maximum_container_depth(&materialized),
            expected_wire_depth,
            "{label}: actual wire Value has the wrong container depth"
        );
        validate_json_value_for_storage(&materialized)
            .unwrap_or_else(|error| panic!("{label}: final Value validation failed: {error}"));

        let bytes = to_deterministic_json_bytes_categorized(&envelope)
            .unwrap_or_else(|error| panic!("{label}: deterministic encode failed: {error}"));
        let decoded = parse_strict_json_object(&bytes)
            .unwrap_or_else(|error| panic!("{label}: strict re-decode failed: {error}"));
        assert_eq!(decoded, materialized);
        assert_eq!(
            to_deterministic_json_bytes_categorized(&decoded)
                .unwrap_or_else(|error| panic!("{label}: deterministic re-encode failed: {error}")),
            bytes,
            "{label}: decode/re-encode changed deterministic bytes"
        );
        bytes
    }

    fn assert_raw_object_success(label: &str, raw: &RawValue, expected_depth: usize) {
        raw.serialize(JsonSerializeValidator::root())
            .unwrap_or_else(|error| panic!("{label}: direct raw preflight failed: {error}"));
        let expected = parse_strict_json_object(raw.get().as_bytes()).unwrap_or_else(|error| {
            panic!("{label}: source RawValue should strict-parse: {error}")
        });
        let materialized = serde_json::to_value(raw)
            .unwrap_or_else(|error| panic!("{label}: raw materialization failed: {error}"));
        assert_eq!(materialized, expected, "{label}: raw meaning changed");
        assert_eq!(
            maximum_container_depth(&materialized),
            expected_depth,
            "{label}: actual RawValue wire has the wrong depth"
        );
        validate_json_value_for_storage(&materialized)
            .unwrap_or_else(|error| panic!("{label}: final Value validation failed: {error}"));

        let bytes = to_deterministic_json_bytes_categorized(raw)
            .unwrap_or_else(|error| panic!("{label}: deterministic encode failed: {error}"));
        let decoded = parse_strict_json_object(&bytes).unwrap_or_else(|error| {
            panic!("{label}: encoded RawValue should strict-parse: {error}")
        });
        assert_eq!(decoded, materialized);
        assert_eq!(
            to_deterministic_json_bytes_categorized(&decoded)
                .unwrap_or_else(|error| panic!("{label}: deterministic re-encode failed: {error}")),
            bytes,
            "{label}: decode/re-encode changed deterministic bytes"
        );
    }

    fn value_with_deep_secret(mut value: Value) -> Value {
        const SECRET: &str = "credential=raw-depth-secret";
        let mut current = &mut value;
        loop {
            match current {
                Value::Object(object) => {
                    current = object
                        .get_mut("value")
                        .expect("nested object fixture should have a value child");
                }
                Value::Array(items) => {
                    current = items
                        .first_mut()
                        .expect("nested array fixture should have one child");
                }
                _ => {
                    *current = Value::String(SECRET.to_owned());
                    break;
                }
            }
        }
        value
    }

    fn raw_value_with_secret(fixture: fn(usize) -> Value) -> (Box<RawValue>, Value) {
        let wire_value = value_with_deep_secret(fixture(MAX_JSON_NESTING_DEPTH + 1));
        let raw = RawValue::from_string(
            serde_json::to_string(&wire_value).expect("raw overflow fixture should serialize"),
        )
        .expect("raw overflow fixture should have valid JSON syntax");
        (raw, wire_value)
    }

    fn assert_raw_depth_overflow(label: &str, raw: &RawValue, wire_value: &Value) {
        let preflight_error = raw
            .serialize(JsonSerializeValidator::root())
            .expect_err("raw depth 128 must fail in direct preflight");
        assert_eq!(
            preflight_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded,
            "{label}: direct preflight returned the wrong category"
        );

        // RawValue는 사전 검사에서 멈춰 materialization에 도달하면 안 된다. 같은 wire
        // 형태를 독립 Value로 구성해 최종 Value validator의 판단만 별도로 검증한다.
        assert_eq!(
            maximum_container_depth(wire_value),
            MAX_JSON_NESTING_DEPTH + 1,
            "{label}: overflow fixture is not at actual wire depth 128"
        );
        let final_error = validate_json_value_for_storage(wire_value)
            .expect_err("materialized raw depth 128 must fail final validation");
        assert_eq!(
            final_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );

        let encode_error = to_deterministic_json_bytes_categorized(raw)
            .expect_err("raw depth 128 must fail before bytes or sorting are produced");
        assert_eq!(
            encode_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );
        for error in [&preflight_error, &final_error, &encode_error] {
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(!rendered.contains("credential=raw-depth-secret"));
            }
            assert!(error.source().is_none());
        }
    }

    fn assert_raw_reserved_key_is_rejected(raw_json: &str) {
        let raw = RawValue::from_string(raw_json.to_owned())
            .expect("reserved-key fixture should have valid raw JSON syntax");
        let preflight_error = raw
            .serialize(JsonSerializeValidator::root())
            .expect_err("direct raw preflight must reject reserved transport keys");
        assert_eq!(
            preflight_error.category(),
            JsonStorageErrorCategory::ReservedObjectKey
        );
        let encode_error = to_deterministic_json_bytes_categorized(raw.as_ref())
            .expect_err("reserved raw key must fail before Value materialization or bytes");
        assert_eq!(
            encode_error.category(),
            JsonStorageErrorCategory::ReservedObjectKey
        );
        for error in [&preflight_error, &encode_error] {
            for rendered in [error.to_string(), format!("{error:?}")] {
                assert!(!rendered.contains("$serde_json::private"));
                assert!(!rendered.contains("FutureTransport"));
            }
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn raw_value_transport_covers_scalar_number_and_all_container_boundaries() {
        let scalar = RawValue::from_string(r#""ordinary scalar""#.to_owned())
            .expect("scalar RawValue should parse");
        assert_raw_envelope_success("scalar", scalar.as_ref(), &json!("ordinary scalar"), 1);

        const LONG_DECIMAL: &str = "0.123456789012345678901234567890123456789012345678901234567890";
        let decimal =
            RawValue::from_string(LONG_DECIMAL.to_owned()).expect("decimal RawValue should parse");
        let expected_decimal: Value =
            serde_json::from_str(LONG_DECIMAL).expect("decimal Value should parse");
        let decimal_bytes =
            assert_raw_envelope_success("decimal", decimal.as_ref(), &expected_decimal, 1);
        assert!(
            std::str::from_utf8(&decimal_bytes)
                .expect("deterministic JSON should be UTF-8")
                .contains(LONG_DECIMAL),
            "arbitrary-precision decimal lexeme changed"
        );

        // 저장 parser가 요구하는 object root 아래에서 array/mixed 경로를 만들고,
        // RawValue 자체의 전체 wire 깊이를 정확히 127로 맞춘다.
        let array = RawValue::from_string(
            String::from_utf8(nested_array(MAX_JSON_NESTING_DEPTH))
                .expect("array fixture should be UTF-8"),
        )
        .expect("array RawValue should parse");
        assert_raw_object_success("array depth 127", array.as_ref(), MAX_JSON_NESTING_DEPTH);

        let mixed = RawValue::from_string(
            String::from_utf8(alternating_containers(MAX_JSON_NESTING_DEPTH))
                .expect("mixed fixture should be UTF-8"),
        )
        .expect("mixed RawValue should parse");
        assert_raw_object_success("mixed depth 127", mixed.as_ref(), MAX_JSON_NESTING_DEPTH);

        let object = RawValue::from_string(
            String::from_utf8(nested_object(MAX_JSON_NESTING_DEPTH))
                .expect("object fixture should be UTF-8"),
        )
        .expect("object RawValue should parse");
        assert_raw_object_success("object depth 127", object.as_ref(), MAX_JSON_NESTING_DEPTH);

        for (label, fixture) in [
            ("object", nested_object_value as fn(usize) -> Value),
            ("array", nested_array_value),
            ("mixed", alternating_container_value),
        ] {
            let (too_deep, wire_value) = raw_value_with_secret(fixture);
            assert_raw_depth_overflow(label, too_deep.as_ref(), &wire_value);
        }
    }

    #[test]
    fn raw_value_transport_rejects_actual_and_escaped_reserved_markers_preflight() {
        for raw_json in [
            r#"{"$serde_json::private::Number":"12345678901234567890"}"#,
            r#"{"$serde_json::private::RawValue":"true"}"#,
            r#"{"$serde_json::private::FutureTransport":{"payload":1}}"#,
            r#"{"\u0024serde_json::private::\u004eumber":"1"}"#,
        ] {
            assert_raw_reserved_key_is_rejected(raw_json);
        }

        let marker_string = RawValue::from_string(r#""$serde_json::private::RawValue""#.to_owned())
            .expect("marker text as a string value should parse");
        assert_raw_envelope_success(
            "marker string value",
            marker_string.as_ref(),
            &Value::String("$serde_json::private::RawValue".to_owned()),
            1,
        );
    }

    #[test]
    fn nesting_budget_is_branch_local_and_errors_are_redacted() {
        let siblings = (0..1_000)
            .map(|index| format!(r#""item{index}":[{{"value":{index}}}]"#))
            .collect::<Vec<_>>()
            .join(",");
        parse_strict_json_object(format!("{{{siblings}}}").as_bytes())
            .expect("wide siblings must not consume one another's depth budget");

        const SECRET: &str = "credential=depth-secret";
        let mut deep = nested_array(MAX_JSON_NESTING_DEPTH + 1);
        let scalar = deep
            .iter()
            .position(|byte| *byte == b'0')
            .expect("fixture must contain a scalar");
        deep.splice(scalar..=scalar, format!(r#""{SECRET}""#).bytes());
        let error = parse_strict_json_object(&deep)
            .expect_err("deep input must return Result::Err without panicking");
        assert_eq!(
            error.category(),
            StrictJsonErrorCategory::NestingDepthExceeded
        );
        let mut current: Option<&(dyn Error + 'static)> = Some(&error);
        while let Some(node) = current {
            assert!(!node.to_string().contains(SECRET));
            assert!(!format!("{node:?}").contains(SECRET));
            current = node.source();
        }
    }

    fn nested_object_value(container_depth: usize) -> Value {
        assert!(container_depth >= 1);
        let mut value = Value::Null;
        for _ in 0..container_depth {
            value = Value::Object(Map::from_iter([("value".to_owned(), value)]));
        }
        value
    }

    fn nested_array_value(container_depth: usize) -> Value {
        assert!(container_depth >= 1);
        let mut value = Value::Null;
        for _ in 0..container_depth {
            value = Value::Array(vec![value]);
        }
        value
    }

    fn alternating_container_value(container_depth: usize) -> Value {
        assert!(container_depth >= 1);
        let mut value = Value::Null;
        for depth in (1..=container_depth).rev() {
            value = if depth % 2 == 0 {
                Value::Array(vec![value])
            } else {
                Value::Object(Map::from_iter([("value".to_owned(), value)]))
            };
        }
        value
    }

    fn maximum_container_depth(value: &Value) -> usize {
        let mut maximum = 0;
        let mut pending = vec![(value, 0usize)];
        while let Some((value, parent_depth)) = pending.pop() {
            match value {
                Value::Object(object) => {
                    let depth = parent_depth.checked_add(1).expect("test depth should fit");
                    maximum = maximum.max(depth);
                    pending.extend(object.values().map(|child| (child, depth)));
                }
                Value::Array(items) => {
                    let depth = parent_depth.checked_add(1).expect("test depth should fit");
                    maximum = maximum.max(depth);
                    pending.extend(items.iter().map(|child| (child, depth)));
                }
                _ => {}
            }
        }
        maximum
    }

    fn assert_depth_error<T>(value: &T)
    where
        T: Serialize + ?Sized,
    {
        let error = to_deterministic_json_bytes_categorized(value)
            .expect_err("depth 128 must fail before bytes are returned");
        assert_eq!(
            error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );
        for rendered in [error.to_string(), format!("{error:?}")] {
            assert!(!rendered.contains("credential=encode-depth-secret"));
        }
        assert!(error.source().is_none());
    }

    #[test]
    fn programmatic_values_use_the_exact_object_array_and_mixed_encode_boundary() {
        for fixture in [
            nested_object_value as fn(usize) -> Value,
            nested_array_value,
            alternating_container_value,
        ] {
            let boundary = fixture(MAX_JSON_NESTING_DEPTH);
            assert_eq!(maximum_container_depth(&boundary), MAX_JSON_NESTING_DEPTH);
            validate_json_value_for_storage(&boundary)
                .expect("materialized Value depth 127 should validate");
            to_deterministic_json_bytes_categorized(&boundary)
                .expect("programmatic Value depth 127 should encode");

            let too_deep = fixture(MAX_JSON_NESTING_DEPTH + 1);
            assert_eq!(
                maximum_container_depth(&too_deep),
                MAX_JSON_NESTING_DEPTH + 1
            );
            assert_eq!(
                validate_json_value_for_storage(&too_deep)
                    .expect_err("materialized Value depth 128 must fail")
                    .category(),
                JsonStorageErrorCategory::NestingDepthExceeded
            );
            assert_depth_error(&too_deep);
        }

        let object = nested_object_value(MAX_JSON_NESTING_DEPTH);
        let bytes = to_deterministic_json_bytes(&object).expect("boundary object should encode");
        parse_strict_json_object(&bytes)
            .expect("every encoded storage object must remain strict-decodable");
    }

    #[test]
    fn scalar_roots_and_wide_shallow_values_do_not_consume_false_depth() {
        for scalar in [
            Value::Null,
            Value::Bool(true),
            Value::String("scalar".to_owned()),
        ] {
            assert_eq!(maximum_container_depth(&scalar), 0);
            validate_json_value_for_storage(&scalar)
                .expect("scalar roots have container depth zero");
            to_deterministic_json_bytes_categorized(&scalar).expect("scalar should encode");
        }

        let wide = Value::Object(Map::from_iter((0..1_000).map(|index| {
            (
                format!("item{index}"),
                Value::Array(vec![Value::Object(Map::from_iter([(
                    "value".to_owned(),
                    Value::from(index),
                )]))]),
            )
        })));
        assert_eq!(maximum_container_depth(&wide), 3);
        to_deterministic_json_bytes_categorized(&wide)
            .expect("wide siblings must not accumulate depth");

        let overflow = JsonSerializeValidator {
            container_depth: usize::MAX,
        }
        .enter_container()
        .expect_err("counter overflow must fail closed");
        assert_eq!(
            overflow.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );
    }

    struct NestedStruct(usize);

    impl Serialize for NestedStruct {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if self.0 == 0 {
                return serializer.serialize_u8(0);
            }
            let child = Self(self.0 - 1);
            let mut state = serializer.serialize_struct("NestedStruct", 1)?;
            state.serialize_field("child", &child)?;
            state.end()
        }
    }

    struct NestedSequence(usize);

    impl Serialize for NestedSequence {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if self.0 == 0 {
                return serializer.serialize_u8(0);
            }
            let child = Self(self.0 - 1);
            let mut state = serializer.serialize_seq(Some(1))?;
            state.serialize_element(&child)?;
            state.end()
        }
    }

    struct NestedMap(usize);

    impl Serialize for NestedMap {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if self.0 == 0 {
                return serializer.serialize_u8(0);
            }
            let child = Self(self.0 - 1);
            let mut state = serializer.serialize_map(Some(1))?;
            state.serialize_entry("child", &child)?;
            state.end()
        }
    }

    #[derive(Serialize)]
    struct TransparentNewtype(NestedStruct);

    #[derive(Serialize)]
    enum NestedVariant {
        Newtype(NestedStruct),
        Tuple(NestedStruct, u8),
        Struct { child: NestedStruct },
    }

    #[test]
    fn generic_serialize_preflight_counts_struct_sequence_map_and_enum_wrappers() {
        to_deterministic_json_bytes_categorized(&NestedStruct(MAX_JSON_NESTING_DEPTH))
            .expect("nested structs at depth 127 should encode");
        assert_depth_error(&NestedStruct(MAX_JSON_NESTING_DEPTH + 1));
        to_deterministic_json_bytes_categorized(&NestedSequence(MAX_JSON_NESTING_DEPTH))
            .expect("nested sequences at depth 127 should encode");
        assert_depth_error(&NestedSequence(MAX_JSON_NESTING_DEPTH + 1));
        to_deterministic_json_bytes_categorized(&NestedMap(MAX_JSON_NESTING_DEPTH))
            .expect("nested maps at depth 127 should encode");
        assert_depth_error(&NestedMap(MAX_JSON_NESTING_DEPTH + 1));
        to_deterministic_json_bytes_categorized(&TransparentNewtype(NestedStruct(
            MAX_JSON_NESTING_DEPTH,
        )))
        .expect("transparent newtypes must not add a JSON container");

        for boundary in [
            NestedVariant::Newtype(NestedStruct(MAX_JSON_NESTING_DEPTH - 1)),
            NestedVariant::Tuple(NestedStruct(MAX_JSON_NESTING_DEPTH - 2), 0),
            NestedVariant::Struct {
                child: NestedStruct(MAX_JSON_NESTING_DEPTH - 2),
            },
        ] {
            to_deterministic_json_bytes_categorized(&boundary)
                .expect("enum wire depth 127 should encode");
        }
        assert_depth_error(&NestedVariant::Newtype(NestedStruct(
            MAX_JSON_NESTING_DEPTH,
        )));
        assert_depth_error(&NestedVariant::Tuple(
            NestedStruct(MAX_JSON_NESTING_DEPTH - 1),
            0,
        ));
        assert_depth_error(&NestedVariant::Struct {
            child: NestedStruct(MAX_JSON_NESTING_DEPTH - 1),
        });
    }

    struct NestedEnvelope<'a, T>
    where
        T: Serialize + ?Sized,
    {
        remaining: usize,
        value: &'a T,
    }

    impl<T> Serialize for NestedEnvelope<'_, T>
    where
        T: Serialize + ?Sized,
    {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            if self.remaining == 0 {
                return self.value.serialize(serializer);
            }
            let child = NestedEnvelope {
                remaining: self.remaining - 1,
                value: self.value,
            };
            let mut state = serializer.serialize_struct("NestedEnvelope", 1)?;
            state.serialize_field("value", &child)?;
            state.end()
        }
    }

    #[derive(Serialize)]
    struct WireTupleStruct(u8, u8);

    #[derive(Serialize)]
    enum WireExternal {
        Unit,
        Newtype((u8, u8)),
        Tuple(u8, u8),
        Struct { child: (u8, u8) },
    }

    #[derive(Serialize)]
    #[serde(tag = "kind")]
    enum WireInternal {
        Struct { child: (u8, u8) },
    }

    #[derive(Serialize)]
    #[serde(tag = "kind", content = "content")]
    enum WireAdjacent {
        Tuple(u8, u8),
    }

    #[derive(Serialize)]
    #[serde(untagged)]
    enum WireUntagged {
        Tuple(u8, u8),
    }

    struct WireBytes<'a>(&'a [u8]);

    impl Serialize for WireBytes<'_> {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_bytes(self.0)
        }
    }

    fn assert_generic_wire_boundary<T>(label: &str, value: &T)
    where
        T: Serialize + ?Sized,
    {
        let base_value = serde_json::to_value(value)
            .unwrap_or_else(|error| panic!("{label}: base wire materialization failed: {error}"));
        let base_depth = maximum_container_depth(&base_value);
        let wrapper_count = MAX_JSON_NESTING_DEPTH
            .checked_sub(base_depth)
            .unwrap_or_else(|| panic!("{label}: base expression already exceeds the depth budget"));

        let boundary = NestedEnvelope {
            remaining: wrapper_count,
            value,
        };
        boundary
            .serialize(JsonSerializeValidator::root())
            .unwrap_or_else(|error| {
                panic!("{label}: direct preflight at depth 127 failed: {error}")
            });
        let materialized = serde_json::to_value(&boundary)
            .unwrap_or_else(|error| panic!("{label}: boundary materialization failed: {error}"));
        assert_eq!(
            maximum_container_depth(&materialized),
            MAX_JSON_NESTING_DEPTH,
            "{label}: actual boundary wire Value is not depth 127"
        );
        validate_json_value_for_storage(&materialized)
            .unwrap_or_else(|error| panic!("{label}: final boundary validation failed: {error}"));

        let bytes = to_deterministic_json_bytes_categorized(&boundary)
            .unwrap_or_else(|error| panic!("{label}: boundary encode failed: {error}"));
        let decoded = parse_strict_json_object(&bytes)
            .unwrap_or_else(|error| panic!("{label}: strict boundary re-decode failed: {error}"));
        assert_eq!(decoded, materialized);
        assert_eq!(
            to_deterministic_json_bytes_categorized(&decoded)
                .unwrap_or_else(|error| panic!("{label}: boundary re-encode failed: {error}")),
            bytes,
            "{label}: deterministic bytes changed after strict re-decode"
        );

        let too_deep = NestedEnvelope {
            remaining: wrapper_count + 1,
            value,
        };
        let preflight_error = too_deep
            .serialize(JsonSerializeValidator::root())
            .expect_err("depth 128 must fail in direct generic preflight");
        assert_eq!(
            preflight_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded,
            "{label}: direct preflight returned the wrong error category"
        );

        // 실제 backend wire Value도 별도로 만들어 최종 검증 단계의 128 거부를 확인한다.
        let too_deep_value = serde_json::to_value(&too_deep)
            .unwrap_or_else(|error| panic!("{label}: depth-128 materialization failed: {error}"));
        assert_eq!(
            maximum_container_depth(&too_deep_value),
            MAX_JSON_NESTING_DEPTH + 1,
            "{label}: actual overflow wire Value is not depth 128"
        );
        let final_error = validate_json_value_for_storage(&too_deep_value)
            .expect_err("materialized generic depth 128 must fail final validation");
        assert_eq!(
            final_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );
        let encode_error = to_deterministic_json_bytes_categorized(&too_deep)
            .expect_err("generic depth 128 must fail before bytes are returned");
        assert_eq!(
            encode_error.category(),
            JsonStorageErrorCategory::NestingDepthExceeded
        );
        for error in [&preflight_error, &final_error, &encode_error] {
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn generic_serde_wire_shapes_share_exact_preflight_and_final_depth_boundaries() {
        let tuple = (1u8, 2u8);
        assert_eq!(serde_json::to_value(tuple).unwrap(), json!([1, 2]));
        assert_generic_wire_boundary("tuple", &tuple);

        let tuple_struct = WireTupleStruct(1, 2);
        assert_eq!(serde_json::to_value(&tuple_struct).unwrap(), json!([1, 2]));
        assert_generic_wire_boundary("tuple struct", &tuple_struct);

        let unit_variant = WireExternal::Unit;
        assert_eq!(serde_json::to_value(&unit_variant).unwrap(), json!("Unit"));
        assert_generic_wire_boundary("unit enum", &unit_variant);

        let external_newtype = WireExternal::Newtype((1, 2));
        assert_eq!(
            serde_json::to_value(&external_newtype).unwrap(),
            json!({"Newtype": [1, 2]})
        );
        assert_generic_wire_boundary("externally tagged newtype", &external_newtype);

        let external_tuple = WireExternal::Tuple(1, 2);
        assert_eq!(
            serde_json::to_value(&external_tuple).unwrap(),
            json!({"Tuple": [1, 2]})
        );
        assert_generic_wire_boundary("externally tagged tuple", &external_tuple);

        let external_struct = WireExternal::Struct { child: (1, 2) };
        assert_eq!(
            serde_json::to_value(&external_struct).unwrap(),
            json!({"Struct": {"child": [1, 2]}})
        );
        assert_generic_wire_boundary("externally tagged struct", &external_struct);

        let internal = WireInternal::Struct { child: (1, 2) };
        assert_eq!(
            serde_json::to_value(&internal).unwrap(),
            json!({"kind": "Struct", "child": [1, 2]})
        );
        assert_generic_wire_boundary("internally tagged enum", &internal);

        let adjacent = WireAdjacent::Tuple(1, 2);
        assert_eq!(
            serde_json::to_value(&adjacent).unwrap(),
            json!({"kind": "Tuple", "content": [1, 2]})
        );
        assert_generic_wire_boundary("adjacently tagged enum", &adjacent);

        let untagged = WireUntagged::Tuple(1, 2);
        assert_eq!(serde_json::to_value(&untagged).unwrap(), json!([1, 2]));
        assert_generic_wire_boundary("untagged enum", &untagged);

        let bytes = WireBytes(&[0, 127, 255]);
        assert_eq!(serde_json::to_value(&bytes).unwrap(), json!([0, 127, 255]));
        assert_generic_wire_boundary("bytes", &bytes);

        let some = Some((1u8, 2u8));
        assert_eq!(serde_json::to_value(some).unwrap(), json!([1, 2]));
        assert_generic_wire_boundary("Some", &some);

        let none = Option::<(u8, u8)>::None;
        assert_eq!(serde_json::to_value(none).unwrap(), Value::Null);
        assert_generic_wire_boundary("None", &none);
    }

    #[derive(Serialize)]
    struct FloatField {
        value: f64,
    }

    #[derive(Serialize)]
    struct FloatNewtype(f64);

    #[derive(Serialize)]
    enum FloatVariant {
        Newtype(f64),
        Tuple(u8, f64),
        Struct { value: f64 },
    }

    #[test]
    fn rejects_non_finite_floats_through_nested_serde_paths() {
        let nested_map =
            BTreeMap::from([("outer", BTreeMap::from([("value", f64::NEG_INFINITY)]))]);

        assert!(to_deterministic_json_string(&FloatField { value: f64::NAN }).is_err());
        assert!(to_deterministic_json_string(&vec![0.0, f64::INFINITY]).is_err());
        assert!(to_deterministic_json_string(&nested_map).is_err());
        assert!(to_deterministic_json_string(&Some(f64::NAN)).is_err());
        assert!(to_deterministic_json_string(&FloatNewtype(f64::INFINITY)).is_err());
        assert!(to_deterministic_json_string(&FloatVariant::Newtype(f64::NAN)).is_err());
        assert!(to_deterministic_json_string(&FloatVariant::Tuple(1, f64::INFINITY)).is_err());
        assert!(to_deterministic_json_string(&FloatVariant::Struct {
            value: f64::NEG_INFINITY,
        })
        .is_err());
    }

    #[test]
    fn null_and_none_remain_valid_null_values() {
        let output = serialize(&json!({"explicit": null, "optional": Option::<u8>::None}));
        let decoded: Value = serde_json::from_str(&output).expect("generated JSON should parse");

        assert!(decoded["explicit"].is_null());
        assert!(decoded["optional"].is_null());
    }

    struct AlwaysFails;

    impl Serialize for AlwaysFails {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(S::Error::custom("intentional serialization failure"))
        }
    }

    #[test]
    fn custom_serialization_error_is_returned() {
        let error = to_deterministic_json_string(&AlwaysFails)
            .expect_err("custom serialization should fail");

        assert!(error
            .to_string()
            .contains("intentional serialization failure"));
    }

    #[test]
    fn repeated_serialization_is_byte_identical() {
        let value = json!({"z": 3, "a": ["하나", "둘"]});

        assert_eq!(serialize(&value).as_bytes(), serialize(&value).as_bytes());
    }

    fn serialize<T>(value: &T) -> String
    where
        T: Serialize + ?Sized,
    {
        to_deterministic_json_string(value).expect("test value should serialize")
    }
}
