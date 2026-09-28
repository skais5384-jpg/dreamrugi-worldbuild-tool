use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use uuid::{Uuid, Variant, Version};

/// 영속 ID의 wire 문자열이 canonical UUID v4 계약을 어길 때의 안전한 오류다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistentIdError {
    InvalidCanonicalFormat,
    Nil,
    NotVersion4,
}

impl fmt::Display for PersistentIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCanonicalFormat => {
                formatter.write_str("persistent ID must be a canonical lowercase hyphenated UUID")
            }
            Self::Nil => formatter.write_str("persistent ID must not be nil"),
            Self::NotVersion4 => formatter.write_str("persistent ID must be UUID v4"),
        }
    }
}

impl std::error::Error for PersistentIdError {}

fn parse_uuid_v4(value: &str) -> Result<Uuid, PersistentIdError> {
    let parsed = Uuid::parse_str(value).map_err(|_| PersistentIdError::InvalidCanonicalFormat)?;
    if parsed.is_nil() {
        return Err(PersistentIdError::Nil);
    }
    if parsed.get_version() != Some(Version::Random) || parsed.get_variant() != Variant::RFC4122 {
        return Err(PersistentIdError::NotVersion4);
    }
    if parsed.hyphenated().to_string() != value {
        return Err(PersistentIdError::InvalidCanonicalFormat);
    }
    Ok(parsed)
}

macro_rules! persistent_id {
    ($name:ident) => {
        /// 표시 이름과 독립적인 장기 영속 entity 식별자다.
        ///
        /// UUID는 16바이트 불변 값이라 `Copy`가 소유권이나 credential 수명을 숨기지 않으며,
        /// 종류별 newtype이 서로 다른 ID의 암시적 대입을 계속 차단한다.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) struct $name(Uuid);

        impl $name {
            /// 운영 데이터에 사용할 충돌 저항성 UUID v4를 생성한다.
            pub(crate) fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub(crate) fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl FromStr for $name {
            type Err = PersistentIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                parse_uuid_v4(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "{}", self.0.hyphenated())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                value.parse().map_err(de::Error::custom)
            }
        }
    };
}

persistent_id!(TemplateId);
persistent_id!(FieldId);
persistent_id!(OptionId);
persistent_id!(DocumentId);
persistent_id!(InstanceId);
persistent_id!(ReferenceId);
