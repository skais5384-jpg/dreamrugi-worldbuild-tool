use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 프로젝트 루트 안의 일반 데이터 대상을 나타내는 검증된 상대 경로다.
///
/// 이 값 타입은 문자열 정규화와 명백한 루트 이탈만 책임진다. symlink·junction 및
/// canonical parent가 실제 프로젝트 경계 안인지 확인하는 일은 경로를 사용하는 I/O
/// 계층의 책임이다.
#[derive(Debug, Clone)]
pub(crate) struct ProjectRelativePath {
    serialized: String,
    comparison_key: String,
}

impl ProjectRelativePath {
    pub(crate) fn parse(value: &str) -> Result<Self, ProjectRelativePathError> {
        if value.is_empty() {
            return Err(ProjectRelativePathError::Empty);
        }

        let normalized = value.replace('\\', "/");
        if normalized.starts_with('/') || has_windows_prefix(&normalized) {
            return Err(ProjectRelativePathError::AbsoluteOrPrefixed);
        }

        let segments: Vec<_> = normalized.split('/').collect();
        if segments.is_empty() || segments.iter().any(|segment| segment.is_empty()) {
            return Err(ProjectRelativePathError::MissingFileNameOrEmptyComponent);
        }
        if segments.contains(&".") {
            return Err(ProjectRelativePathError::CurrentDirectoryComponent);
        }
        if segments.contains(&"..") {
            return Err(ProjectRelativePathError::ParentDirectoryComponent);
        }
        if segments
            .first()
            .is_some_and(|segment| segment.eq_ignore_ascii_case(".worldbuild"))
        {
            return Err(ProjectRelativePathError::ReservedSystemDirectory);
        }

        let serialized = segments.join("/");
        // 일반 Windows 파일시스템의 구분자와 ASCII 대소문자 별칭은 같은 대상으로
        // 비교한다. case-sensitive 특수 디렉터리에서는 보수적인 중복 거부가 가능하다.
        #[cfg(windows)]
        let comparison_key = serialized.to_ascii_lowercase();
        #[cfg(not(windows))]
        let comparison_key = serialized.clone();

        Ok(Self {
            serialized,
            comparison_key,
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.serialized
    }
}

fn has_windows_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectRelativePathError {
    Empty,
    AbsoluteOrPrefixed,
    MissingFileNameOrEmptyComponent,
    CurrentDirectoryComponent,
    ParentDirectoryComponent,
    ReservedSystemDirectory,
}

impl fmt::Display for ProjectRelativePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self {
            Self::Empty => "path is empty",
            Self::AbsoluteOrPrefixed => {
                "absolute, rooted, and Windows-prefixed paths are not allowed"
            }
            Self::MissingFileNameOrEmptyComponent => {
                "path must contain a file name and no empty components"
            }
            Self::CurrentDirectoryComponent => "current-directory components are not allowed",
            Self::ParentDirectoryComponent => "parent-directory components are not allowed",
            Self::ReservedSystemDirectory => "the .worldbuild system directory is reserved",
        };
        formatter.write_str(reason)
    }
}

impl std::error::Error for ProjectRelativePathError {}

impl fmt::Display for ProjectRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.serialized)
    }
}

impl PartialEq for ProjectRelativePath {
    fn eq(&self, other: &Self) -> bool {
        self.comparison_key == other.comparison_key
    }
}

impl Eq for ProjectRelativePath {}

impl PartialOrd for ProjectRelativePath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProjectRelativePath {
    fn cmp(&self, other: &Self) -> Ordering {
        self.comparison_key.cmp(&other.comparison_key)
    }
}

impl Serialize for ProjectRelativePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.serialized)
    }
}

impl<'de> Deserialize<'de> for ProjectRelativePath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_nested_and_korean_paths() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProjectRelativePath::parse("file.json")?.as_str(),
            "file.json"
        );
        assert_eq!(
            ProjectRelativePath::parse("data/characters/hero.json")?.as_str(),
            "data/characters/hero.json"
        );
        assert_eq!(
            ProjectRelativePath::parse("자료/세계/설정.json")?.as_str(),
            "자료/세계/설정.json"
        );
        Ok(())
    }

    #[test]
    fn normalizes_backslashes_and_displays_only_validated_relative_path(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = ProjectRelativePath::parse(r"data\characters\hero.json")?;
        assert_eq!(path.as_str(), "data/characters/hero.json");
        assert_eq!(path.to_string(), "data/characters/hero.json");
        Ok(())
    }

    #[test]
    fn rejects_invalid_and_filename_less_paths_without_echoing_input() {
        for value in [
            "",
            "/",
            "/data/file.json",
            r"C:\data\file.json",
            r"C:relative.json",
            r"\\server\share\file.json",
            "folder/",
            "folder//file.json",
            ".",
            "data/./file.json",
            "..",
            "data/../file.json",
            ".worldbuild",
            ".worldbuild/transactions/file.json",
            ".WORLDBUILD/state.json",
        ] {
            ProjectRelativePath::parse(value).expect_err("path must be rejected");
        }

        let absolute = r"C:\private-project\secret.json";
        let error = ProjectRelativePath::parse(absolute).expect_err("absolute path must fail");
        assert!(!error.to_string().contains(absolute));
    }

    #[test]
    fn comparison_uses_platform_path_alias_rules() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProjectRelativePath::parse("data/file.json")?,
            ProjectRelativePath::parse(r"data\file.json")?
        );
        #[cfg(windows)]
        assert_eq!(
            ProjectRelativePath::parse("DATA/File.JSON")?,
            ProjectRelativePath::parse("data/file.json")?
        );
        #[cfg(not(windows))]
        assert_ne!(
            ProjectRelativePath::parse("DATA/File.JSON")?,
            ProjectRelativePath::parse("data/file.json")?
        );
        Ok(())
    }

    #[test]
    fn serde_round_trip_uses_normalized_string_and_revalidates_input(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = ProjectRelativePath::parse(r"자료\세계\설정.json")?;
        let bytes = serde_json::to_vec(&path)?;
        assert_eq!(bytes, r#""자료/세계/설정.json""#.as_bytes());
        assert_eq!(serde_json::from_slice::<ProjectRelativePath>(&bytes)?, path);
        assert!(serde_json::from_str::<ProjectRelativePath>(r#""../outside.json""#).is_err());
        Ok(())
    }
}
