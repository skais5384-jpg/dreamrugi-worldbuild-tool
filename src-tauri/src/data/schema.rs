use std::fmt;

use serde::{Deserialize, Serialize};

/// 현재 애플리케이션이 새 프로젝트를 만들 때 사용하는 스키마 버전이다.
pub const CURRENT_SCHEMA_VERSION: SchemaVersion = SchemaVersion::new_unchecked(1);

/// 저장 데이터의 구조 버전을 나타낸다.
///
/// 일반 `u32`를 그대로 사용하지 않고 별도 타입으로 감싸서,
/// 유효하지 않은 0이 저장 데이터에 섞이는 일을 방지한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    /// 상수 선언에만 사용하는 생성자다.
    ///
    /// 런타임 입력에는 검증을 수행하는 `try_from`을 사용해야 한다.
    pub(crate) const fn new_unchecked(value: u32) -> Self {
        Self(value)
    }

    /// 비교나 진단 출력에 사용할 원시 버전 값을 반환한다.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<u32> for SchemaVersion {
    type Error = SchemaVersionError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(SchemaVersionError::Zero);
        }

        Ok(Self(value))
    }
}

impl From<SchemaVersion> for u32 {
    fn from(version: SchemaVersion) -> Self {
        version.get()
    }
}

/// 스키마 버전 값이 유효하지 않을 때 반환하는 오류다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaVersionError {
    Zero,
}

impl fmt::Display for SchemaVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("schema version must be greater than zero"),
        }
    }
}

impl std::error::Error for SchemaVersionError {}

/// 전체 파일을 해석하기 전에 `schemaVersion`만 확인할 때 사용하는 최소 구조다.
///
/// 알 수 없는 다른 필드는 허용한다. 그래야 현재 버전보다 새로운 파일도
/// 내용을 전부 해석하지 않고 버전부터 안전하게 판정할 수 있다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaHeader {
    pub schema_version: SchemaVersion,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_rejects_zero() {
        let result = SchemaVersion::try_from(0);

        assert_eq!(result, Err(SchemaVersionError::Zero));
    }

    #[test]
    fn schema_version_uses_a_plain_json_number() {
        let json = serde_json::to_string(&CURRENT_SCHEMA_VERSION)
            .expect("test schema version should serialize");

        assert_eq!(json, "1");
    }

    #[test]
    fn schema_header_reads_future_version_without_rejecting_other_fields() {
        let json = r#"{"schemaVersion":2,"futureField":true}"#;

        let header: SchemaHeader =
            serde_json::from_str(json).expect("schema header should deserialize");

        assert_eq!(header.schema_version.get(), 2);
    }

    #[test]
    fn schema_header_rejects_zero_from_json() {
        let json = r#"{"schemaVersion":0}"#;

        let result = serde_json::from_str::<SchemaHeader>(json);

        assert!(result.is_err());
    }
}
