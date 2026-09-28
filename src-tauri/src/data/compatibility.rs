use std::{error::Error, fmt};

use serde::{
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor},
    Deserialize,
};
use serde_json::{value::RawValue, Number};

use super::json::inspect_reserved_json_object_keys;
use super::migration::ValidatedRegistrySupport;
use super::schema::SchemaVersion;

const SCHEMA_VERSION_FIELD: &str = "schemaVersion";

/// 일반 프로젝트 데이터가 현재 reader/writer와 어떤 관계인지 나타낸다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaCompatibility {
    Current {
        version: SchemaVersion,
    },
    MigrationRequired {
        found: SchemaVersion,
        current: SchemaVersion,
        minimum_migratable: SchemaVersion,
    },
    UnsupportedFuture {
        found: SchemaVersion,
        current: SchemaVersion,
    },
    UnsupportedPast {
        found: SchemaVersion,
        minimum_migratable: SchemaVersion,
    },
}

/// 지원 범위를 한곳에서 검증하고 판정하기 위한 정책이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SchemaSupportPolicy {
    current: SchemaVersion,
    minimum_migratable: SchemaVersion,
}

impl SchemaSupportPolicy {
    fn try_new(current: u32, minimum_migratable: u32) -> Result<Self, SchemaCompatibilityError> {
        let current = SchemaVersion::try_from(current)
            .map_err(|_| SchemaCompatibilityError::policy("current version must be positive"))?;
        let minimum_migratable = SchemaVersion::try_from(minimum_migratable).map_err(|_| {
            SchemaCompatibilityError::policy("minimum migratable version must be positive")
        })?;
        if minimum_migratable > current {
            return Err(SchemaCompatibilityError::policy(
                "minimum migratable version must not exceed current version",
            ));
        }
        Ok(Self {
            current,
            minimum_migratable,
        })
    }

    /// Production 정책은 검증을 마친 migration registry의 증표로만 생성한다.
    pub(super) fn from_validated_registry(support: ValidatedRegistrySupport) -> Self {
        Self {
            current: support.current(),
            minimum_migratable: support.minimum_migratable(),
        }
    }

    pub(crate) const fn current(self) -> SchemaVersion {
        self.current
    }

    pub(crate) const fn minimum_migratable(self) -> SchemaVersion {
        self.minimum_migratable
    }

    pub(crate) fn classify(self, found: SchemaVersion) -> SchemaCompatibility {
        if found == self.current {
            SchemaCompatibility::Current { version: found }
        } else if found > self.current {
            SchemaCompatibility::UnsupportedFuture {
                found,
                current: self.current,
            }
        } else if found >= self.minimum_migratable {
            SchemaCompatibility::MigrationRequired {
                found,
                current: self.current,
                minimum_migratable: self.minimum_migratable,
            }
        } else {
            SchemaCompatibility::UnsupportedPast {
                found,
                minimum_migratable: self.minimum_migratable,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaCompatibilityErrorCategory {
    MalformedJson,
    InvalidEncoding,
    RootTypeMismatch,
    MissingSchemaVersion,
    DuplicateSchemaVersion,
    InvalidSchemaVersionType,
    InvalidSchemaVersionValue,
    SchemaVersionOutOfRange,
    InvalidSupportPolicy,
}

/// 오류에는 판정 category와 원인만 담고 입력 본문이나 경로는 담지 않는다.
#[derive(Debug)]
pub(crate) struct SchemaCompatibilityError {
    category: SchemaCompatibilityErrorCategory,
    detail: &'static str,
    source: Option<serde_json::Error>,
}

impl SchemaCompatibilityError {
    pub(crate) fn category(&self) -> SchemaCompatibilityErrorCategory {
        self.category
    }

    fn policy(detail: &'static str) -> Self {
        Self {
            category: SchemaCompatibilityErrorCategory::InvalidSupportPolicy,
            detail,
            source: None,
        }
    }

    fn input(category: SchemaCompatibilityErrorCategory, detail: &'static str) -> Self {
        Self {
            category,
            detail,
            source: None,
        }
    }

    fn json(
        category: SchemaCompatibilityErrorCategory,
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

impl fmt::Display for SchemaCompatibilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "schema compatibility inspection failed ({:?}): {}",
            self.category, self.detail
        )
    }
}

impl Error for SchemaCompatibilityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &(dyn Error + 'static))
    }
}

/// 파일이나 transaction에 접근하지 않고 일반 데이터의 최소 header만 판정한다.
pub(crate) fn inspect_schema_compatibility(
    bytes: &[u8],
    policy: SchemaSupportPolicy,
) -> Result<SchemaCompatibility, SchemaCompatibilityError> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(SchemaCompatibilityError::input(
            SchemaCompatibilityErrorCategory::InvalidEncoding,
            "UTF-8 BOM is not allowed",
        ));
    }
    std::str::from_utf8(bytes).map_err(|_| {
        SchemaCompatibilityError::input(
            SchemaCompatibilityErrorCategory::InvalidEncoding,
            "input must be valid UTF-8 without BOM",
        )
    })?;

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let version = HeaderSeed
        .deserialize(&mut deserializer)
        .map_err(map_reader_error)?;
    deserializer.end().map_err(|error| {
        SchemaCompatibilityError::json(
            SchemaCompatibilityErrorCategory::MalformedJson,
            "JSON contains trailing data",
            error,
        )
    })?;
    // Header에 없는 unknown 값은 계속 materialize하지 않되, 현재 JSON backend의 private
    // transport namespace와 충돌하는 실제 object key를 공통 경계로 전 깊이에서 차단한다.
    inspect_reserved_json_object_keys(bytes).map_err(|_| {
        SchemaCompatibilityError::input(
            SchemaCompatibilityErrorCategory::MalformedJson,
            "JSON contains a reserved object key",
        )
    })?;
    Ok(policy.classify(version))
}

struct HeaderSeed;

impl<'de> DeserializeSeed<'de> for HeaderSeed {
    type Value = SchemaVersion;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(HeaderVisitor)
    }
}

struct HeaderVisitor;

impl<'de> Visitor<'de> for HeaderVisitor {
    type Value = SchemaVersion;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SCHEMA_ROOT_OBJECT")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut version = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == SCHEMA_VERSION_FIELD {
                if version.is_some() {
                    return Err(de::Error::custom("SCHEMA_DUPLICATE"));
                }
                version = Some(map.next_value_seed(VersionSeed)?);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        version.ok_or_else(|| de::Error::custom("SCHEMA_MISSING"))
    }
}

struct VersionSeed;

impl<'de> DeserializeSeed<'de> for VersionSeed {
    type Value = SchemaVersion;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // RawValue로 원본 token 종류를 먼저 확인해야 실제 object가 arbitrary_precision의
        // 내부 number 운반 표현으로 오인되지 않는다.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let first = raw.get().bytes().find(|byte| !byte.is_ascii_whitespace());
        if !matches!(first, Some(b'-' | b'0'..=b'9')) {
            return Err(de::Error::custom("SCHEMA_VERSION_TYPE"));
        }
        let number: Number = serde_json::from_str(raw.get())
            .map_err(|_| de::Error::custom("SCHEMA_VERSION_VALUE"))?;
        if let Some(value) = number.as_u64() {
            let value =
                u32::try_from(value).map_err(|_| de::Error::custom("SCHEMA_VERSION_RANGE"))?;
            return SchemaVersion::try_from(value)
                .map_err(|_| de::Error::custom("SCHEMA_VERSION_VALUE"));
        }

        let number = number.to_string();
        if number.bytes().all(|byte| byte.is_ascii_digit()) {
            Err(de::Error::custom("SCHEMA_VERSION_RANGE"))
        } else {
            Err(de::Error::custom("SCHEMA_VERSION_VALUE"))
        }
    }
}

fn map_reader_error(source: serde_json::Error) -> SchemaCompatibilityError {
    let message = source.to_string();
    let (category, detail) = if message.contains("SCHEMA_ROOT_OBJECT") {
        (
            SchemaCompatibilityErrorCategory::RootTypeMismatch,
            "JSON root must be an object",
        )
    } else if message.contains("SCHEMA_MISSING") {
        (
            SchemaCompatibilityErrorCategory::MissingSchemaVersion,
            "schemaVersion is required",
        )
    } else if message.contains("SCHEMA_DUPLICATE") {
        (
            SchemaCompatibilityErrorCategory::DuplicateSchemaVersion,
            "schemaVersion must occur exactly once",
        )
    } else if message.contains("SCHEMA_VERSION_RANGE") {
        (
            SchemaCompatibilityErrorCategory::SchemaVersionOutOfRange,
            "schemaVersion exceeds the u32 range",
        )
    } else if message.contains("SCHEMA_VERSION_VALUE") {
        (
            SchemaCompatibilityErrorCategory::InvalidSchemaVersionValue,
            "schemaVersion must be a positive integer",
        )
    } else if message.contains("SCHEMA_VERSION_TYPE") {
        (
            SchemaCompatibilityErrorCategory::InvalidSchemaVersionType,
            "schemaVersion must be a JSON number",
        )
    } else {
        (
            SchemaCompatibilityErrorCategory::MalformedJson,
            "JSON could not be parsed",
        )
    };
    SchemaCompatibilityError::json(category, detail, source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> SchemaSupportPolicy {
        SchemaSupportPolicy::try_new(3, 2).expect("test policy should be valid")
    }

    fn inspect(json: &str) -> Result<SchemaCompatibility, SchemaCompatibilityError> {
        inspect_schema_compatibility(json.as_bytes(), policy())
    }

    fn assert_category(json: &str, expected: SchemaCompatibilityErrorCategory) {
        let error = inspect(json).expect_err("input should be rejected");
        assert_eq!(error.category(), expected);
        assert!(error.source().is_some());
    }

    #[test]
    fn classifies_all_supported_relationships() {
        assert_eq!(
            inspect(r#"{"schemaVersion":3}"#).unwrap(),
            SchemaCompatibility::Current {
                version: SchemaVersion::try_from(3).unwrap()
            }
        );
        assert!(matches!(
            inspect(r#"{"schemaVersion":2}"#).unwrap(),
            SchemaCompatibility::MigrationRequired { .. }
        ));
        assert!(matches!(
            inspect(r#"{"schemaVersion":1}"#).unwrap(),
            SchemaCompatibility::UnsupportedPast { .. }
        ));
        assert!(matches!(
            inspect(r#"{"schemaVersion":4}"#).unwrap(),
            SchemaCompatibility::UnsupportedFuture { .. }
        ));
    }

    #[test]
    fn reads_only_header_despite_future_fields_order_whitespace_and_korean() {
        let inputs = [
            r#"{"schemaVersion":4,"future":{"unknown":true}}"#,
            "{\n  \"이름\": \"세계\",\n  \"schemaVersion\": 4\n}\n",
            r#"{ "futureArray" : [1,2], "schemaVersion" : 4 }"#,
        ];
        for input in inputs {
            assert!(matches!(
                inspect(input).unwrap(),
                SchemaCompatibility::UnsupportedFuture { .. }
            ));
        }
        assert_eq!(inspect(inputs[0]).unwrap(), inspect(inputs[0]).unwrap());
    }

    #[test]
    fn rejects_encoding_and_malformed_json() {
        let invalid_utf8 = inspect_schema_compatibility(&[0xff], policy()).unwrap_err();
        assert_eq!(
            invalid_utf8.category(),
            SchemaCompatibilityErrorCategory::InvalidEncoding
        );
        let bom = inspect_schema_compatibility(b"\xef\xbb\xbf{}", policy()).unwrap_err();
        assert_eq!(
            bom.category(),
            SchemaCompatibilityErrorCategory::InvalidEncoding
        );
        assert_category("", SchemaCompatibilityErrorCategory::MalformedJson);
        assert_category(
            r#"{"schemaVersion":1"#,
            SchemaCompatibilityErrorCategory::MalformedJson,
        );
    }

    #[test]
    fn rejects_non_object_missing_and_duplicate_headers() {
        for input in ["null", "[]", "true", "1", r#""text""#] {
            assert_category(input, SchemaCompatibilityErrorCategory::RootTypeMismatch);
        }
        assert_category(
            r#"{"name":"세계"}"#,
            SchemaCompatibilityErrorCategory::MissingSchemaVersion,
        );
        assert_category(
            r#"{"schemaVersion":2,"schemaVersion":3}"#,
            SchemaCompatibilityErrorCategory::DuplicateSchemaVersion,
        );
    }

    #[test]
    fn rejects_invalid_schema_version_types() {
        for input in [
            r#"{"schemaVersion":null}"#,
            r#"{"schemaVersion":"1"}"#,
            r#"{"schemaVersion":true}"#,
            r#"{"schemaVersion":[]}"#,
            r#"{"schemaVersion":{}}"#,
        ] {
            assert_category(
                input,
                SchemaCompatibilityErrorCategory::InvalidSchemaVersionType,
            );
        }
    }

    #[test]
    fn raw_schema_token_and_reserved_object_key_cannot_be_confused() {
        assert!(matches!(
            inspect(r#"{"schemaVersion":3}"#).unwrap(),
            SchemaCompatibility::Current { .. }
        ));
        for input in [
            r#"{"schemaVersion":{"$serde_json::private::Number":"3"}}"#,
            r#"{"schemaVersion":{"$serde_json::private::RawValue":"3"}}"#,
            r#"{"schemaVersion":{"\u0024serde_json::private::\u004eumber":"3"}}"#,
            r#"{"schemaVersion":{"\u0024serde_json::private::\u0052awValue":"3"}}"#,
            r#"{"schemaVersion":{"$serde_json::private::FutureTransport":"3"}}"#,
        ] {
            assert_category(
                input,
                SchemaCompatibilityErrorCategory::InvalidSchemaVersionType,
            );
        }
        for input in [
            r#"{"schemaVersion":3,"future":{"nested":{"$serde_json::private::Number":"1"}}}"#,
            r#"{"schemaVersion":3,"future":{"nested":{"$serde_json::private::RawValue":"1"}}}"#,
            r#"{"schemaVersion":3,"future":[{"\u0024serde_json::private::\u0052awValue":"1"}]}"#,
            r#"{"schemaVersion":3,"future":{"$serde_json::private::FutureTransport":"1"}}"#,
        ] {
            let error = inspect(input).expect_err("reserved object keys must fail closed");
            assert_eq!(
                error.category(),
                SchemaCompatibilityErrorCategory::MalformedJson
            );
        }
        for input in [
            r#"{"schemaVersion":3,"future":"$serde_json::private::Number"}"#,
            r#"{"schemaVersion":3,"future":{"$serde_json::private":"1","ordinaryFuture":"2"}}"#,
        ] {
            assert!(matches!(
                inspect(input).unwrap(),
                SchemaCompatibility::Current { .. }
            ));
        }
    }

    fn compatibility_with_array_depth(container_depth: usize) -> String {
        assert!(container_depth >= 1);
        let mut json = String::from(r#"{"schemaVersion":3,"future":"#);
        for _ in 1..container_depth {
            json.push('[');
        }
        json.push('0');
        for _ in 1..container_depth {
            json.push(']');
        }
        json.push('}');
        json
    }

    #[test]
    fn compatibility_maps_the_global_nesting_budget_to_malformed_json() {
        let boundary = compatibility_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH);
        assert!(matches!(
            inspect(&boundary).expect("documented depth boundary should parse"),
            SchemaCompatibility::Current { .. }
        ));

        let too_deep =
            compatibility_with_array_depth(crate::data::json::MAX_JSON_NESTING_DEPTH + 1);
        let error = inspect(&too_deep).expect_err("depth overflow must fail closed");
        assert_eq!(
            error.category(),
            SchemaCompatibilityErrorCategory::MalformedJson
        );
    }

    #[test]
    fn rejects_invalid_schema_version_values_and_range() {
        for input in [
            r#"{"schemaVersion":-1}"#,
            r#"{"schemaVersion":0}"#,
            r#"{"schemaVersion":1.5}"#,
            r#"{"schemaVersion":1e0}"#,
        ] {
            assert_category(
                input,
                SchemaCompatibilityErrorCategory::InvalidSchemaVersionValue,
            );
        }
        assert!(matches!(
            inspect(r#"{"schemaVersion":4294967295}"#).unwrap(),
            SchemaCompatibility::UnsupportedFuture { .. }
        ));
        assert_category(
            r#"{"schemaVersion":4294967296}"#,
            SchemaCompatibilityErrorCategory::SchemaVersionOutOfRange,
        );
        assert_category(
            r#"{"schemaVersion":1234567890123456789012345678901234567890}"#,
            SchemaCompatibilityErrorCategory::SchemaVersionOutOfRange,
        );
    }

    #[test]
    fn rejects_invalid_policies_without_correction() {
        for result in [
            SchemaSupportPolicy::try_new(0, 1),
            SchemaSupportPolicy::try_new(1, 0),
            SchemaSupportPolicy::try_new(1, 2),
        ] {
            assert_eq!(
                result.unwrap_err().category(),
                SchemaCompatibilityErrorCategory::InvalidSupportPolicy
            );
        }
    }

    #[test]
    fn diagnostics_do_not_echo_input_or_paths() {
        let secret = r#"{"secret":"private body","schemaVersion":0,"path":"C:\\Users\\name"}"#;
        let display = inspect(secret).unwrap_err().to_string();
        assert!(!display.contains("private body"));
        assert!(!display.contains("C:\\Users"));
        assert!(!display.contains(secret));
    }
}
