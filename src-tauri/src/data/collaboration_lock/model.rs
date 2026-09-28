use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::super::project_relative_path::ProjectRelativePath;
use super::super::utc_time::now_utc_milliseconds;

const SESSION_ID_PREFIX: &str = "lock-";
const SESSION_ID_VERSION: &[u8] = b"worldbuild-lock-session-v1";
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockProviderKind {
    None,
    Svn,
}

impl fmt::Display for LockProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Svn => "svn",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LockCapabilities {
    pub(crate) distributed: bool,
    pub(crate) validation: bool,
    pub(crate) owner_diagnostics: bool,
    pub(crate) steal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LockProviderInfo {
    pub(crate) kind: LockProviderKind,
    pub(crate) capabilities: LockCapabilities,
}

/// 인증 주체 원문이 아니라 provider가 UI·진단용으로 안전하게 정리한 짧은 식별자다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SafeOwnerIdentity(String);

impl SafeOwnerIdentity {
    pub(crate) fn parse(value: &str) -> Result<Self, LockSetIdentityError> {
        if value.is_empty()
            || value.len() > 128
            || value
                .chars()
                .any(|character| character.is_control() || matches!(character, '?' | '&' | '#'))
        {
            return Err(LockSetIdentityError::InvalidSafeOwnerIdentity);
        }
        Ok(Self(value.to_owned()))
    }
}

impl fmt::Display for SafeOwnerIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 잠금 소유권과 진단 사건을 연결하는 opaque ID이며 인증 token으로 사용하지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockSessionId(String);

impl LockSessionId {
    pub(crate) fn generate() -> Result<Self, LockSetIdentityError> {
        let now = now_utc_milliseconds().map_err(|_| LockSetIdentityError::TimeUnavailable)?;
        let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut hasher = Sha256::new();
        hasher.update(SESSION_ID_VERSION);
        hasher.update([0]);
        hasher.update(now.as_bytes());
        hasher.update([0]);
        hasher.update(std::process::id().to_le_bytes());
        hasher.update(counter.to_le_bytes());
        Ok(Self(format!(
            "{SESSION_ID_PREFIX}{}",
            lowercase_hex(&hasher.finalize())
        )))
    }

    pub(crate) fn parse(value: &str) -> Result<Self, LockSetIdentityError> {
        let Some(hex) = value.strip_prefix(SESSION_ID_PREFIX) else {
            return Err(LockSetIdentityError::InvalidSessionId);
        };
        if !is_lowercase_sha256(hex) {
            return Err(LockSetIdentityError::InvalidSessionId);
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LockSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockSetIdentityError {
    InvalidProjectFingerprint,
    InvalidSessionId,
    InvalidSafeOwnerIdentity,
    TimeUnavailable,
}

impl fmt::Display for LockSetIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidProjectFingerprint => "project fingerprint has an invalid format",
            Self::InvalidSessionId => "lock session ID has an invalid format",
            Self::InvalidSafeOwnerIdentity => "safe owner identity has an invalid format",
            Self::TimeUnavailable => "lock session time is unavailable",
        })
    }
}

impl std::error::Error for LockSetIdentityError {}

pub(crate) struct LockAcquireRequest<'a> {
    project_fingerprint: &'a str,
    session_id: &'a LockSessionId,
    target: &'a ProjectRelativePath,
}

impl<'a> LockAcquireRequest<'a> {
    pub(crate) fn new(
        project_fingerprint: &'a str,
        session_id: &'a LockSessionId,
        target: &'a ProjectRelativePath,
    ) -> Result<Self, LockSetIdentityError> {
        validate_project_fingerprint(project_fingerprint)?;
        Ok(Self {
            project_fingerprint,
            session_id,
            target,
        })
    }

    pub(crate) fn project_fingerprint(&self) -> &str {
        self.project_fingerprint
    }

    pub(crate) fn session_id(&self) -> &LockSessionId {
        self.session_id
    }

    pub(crate) fn target(&self) -> &ProjectRelativePath {
        self.target
    }
}

pub(crate) fn validate_project_fingerprint(value: &str) -> Result<(), LockSetIdentityError> {
    if is_lowercase_sha256(value) {
        Ok(())
    } else {
        Err(LockSetIdentityError::InvalidProjectFingerprint)
    }
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn lowercase_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockOperation {
    Acquire,
    Validate,
    Release,
}

impl fmt::Display for LockOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Acquire => "acquire",
            Self::Validate => "validate",
            Self::Release => "release",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockErrorCategory {
    InvalidLockTarget,
    DuplicateLockTarget,
    EmptyLockTargetSet,
    ProviderUnavailable,
    AuthenticationRequired,
    AlreadyLocked,
    StaleLockToken,
    LockAcquireFailed,
    LockValidationFailed,
    LockLost,
    LockStateUnknown,
    LockReleaseFailed,
    HeldLockProviderMismatch,
    HeldLockAlreadyReleased,
    PartialAcquireRolledBack,
    PartialAcquireReleaseFailed,
    LockSetNotActive,
}

impl fmt::Display for LockErrorCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LockError {
    pub(crate) category: LockErrorCategory,
    pub(crate) provider: LockProviderKind,
    pub(crate) operation: LockOperation,
    context: Box<LockErrorContext>,
}

#[derive(Debug, Clone)]
struct LockErrorContext {
    project_fingerprint: Option<String>,
    session_id: Option<LockSessionId>,
    target: Option<ProjectRelativePath>,
    owner: Option<SafeOwnerIdentity>,
}

impl LockError {
    pub(crate) fn for_context(
        category: LockErrorCategory,
        provider: LockProviderKind,
        operation: LockOperation,
        project_fingerprint: Option<&str>,
        session_id: Option<&LockSessionId>,
        target: Option<&ProjectRelativePath>,
    ) -> Self {
        Self {
            category,
            provider,
            operation,
            context: Box::new(LockErrorContext {
                project_fingerprint: project_fingerprint.map(str::to_owned),
                session_id: session_id.cloned(),
                target: target.cloned(),
                owner: None,
            }),
        }
    }

    pub(crate) fn for_request(
        category: LockErrorCategory,
        provider: LockProviderKind,
        operation: LockOperation,
        request: &LockAcquireRequest<'_>,
    ) -> Self {
        Self {
            category,
            provider,
            operation,
            context: Box::new(LockErrorContext {
                project_fingerprint: Some(request.project_fingerprint().to_owned()),
                session_id: Some(request.session_id().clone()),
                target: Some(request.target().clone()),
                owner: None,
            }),
        }
    }

    pub(crate) fn for_held(
        category: LockErrorCategory,
        provider: LockProviderKind,
        operation: LockOperation,
        project_fingerprint: &str,
        session_id: &LockSessionId,
        target: &ProjectRelativePath,
    ) -> Self {
        Self {
            category,
            provider,
            operation,
            context: Box::new(LockErrorContext {
                project_fingerprint: Some(project_fingerprint.to_owned()),
                session_id: Some(session_id.clone()),
                target: Some(target.clone()),
                owner: None,
            }),
        }
    }

    pub(crate) fn category(&self) -> LockErrorCategory {
        self.category
    }

    pub(crate) fn project_fingerprint(&self) -> Option<&str> {
        self.context.project_fingerprint.as_deref()
    }

    pub(crate) fn session_id(&self) -> Option<&LockSessionId> {
        self.context.session_id.as_ref()
    }

    pub(crate) fn target(&self) -> Option<&ProjectRelativePath> {
        self.context.target.as_ref()
    }

    #[cfg(test)]
    pub(super) fn with_owner(mut self, owner: SafeOwnerIdentity) -> Self {
        self.context.owner = Some(owner);
        self
    }

    pub(crate) fn owner(&self) -> Option<&SafeOwnerIdentity> {
        self.context.owner.as_ref()
    }
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} during {} with provider {}",
            self.category, self.operation, self.provider
        )?;
        if let Some(target) = &self.context.target {
            write!(formatter, ", target '{target}'")?;
        }
        if let Some(fingerprint) = &self.context.project_fingerprint {
            write!(formatter, ", project {fingerprint}")?;
        }
        if let Some(session) = &self.context.session_id {
            write!(formatter, ", session {session}")?;
        }
        if let Some(owner) = &self.context.owner {
            write!(formatter, ", owner {owner}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LockError {}
