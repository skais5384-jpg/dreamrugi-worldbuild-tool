use std::fmt;

use serde::{Deserialize, Serialize};

/// Template 내용 이력을 나타내며 artifact schema 버전과 완전히 별개다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub(crate) struct TemplateRevision(u32);

impl TemplateRevision {
    pub(crate) const INITIAL: Self = Self(1);

    pub(crate) const fn get(self) -> u32 {
        self.0
    }

    pub(crate) fn checked_increment(self) -> Result<Self, TemplateRevisionError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(TemplateRevisionError::Overflow)
    }
}

impl TryFrom<u32> for TemplateRevision {
    type Error = TemplateRevisionError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value == 0 {
            return Err(TemplateRevisionError::Zero);
        }
        Ok(Self(value))
    }
}

impl From<TemplateRevision> for u32 {
    fn from(revision: TemplateRevision) -> Self {
        revision.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TemplateRevisionError {
    Zero,
    Overflow,
}

impl fmt::Display for TemplateRevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => formatter.write_str("template revision must be greater than zero"),
            Self::Overflow => formatter.write_str("template revision cannot be incremented"),
        }
    }
}

impl std::error::Error for TemplateRevisionError {}
