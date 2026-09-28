use std::fmt;

use super::super::project_relative_path::ProjectRelativePath;
use super::{LockError, LockProviderKind, LockSessionId, LockSetGuard, LockSetState};

/// 살아 있는 잠금 집합을 독점 차용하는 쓰기 권한이다.
///
/// ID를 복사한 token이 아니므로 permit이 존재하는 동안 기반 guard를 release하거나
/// handle을 교체할 수 없다. 생성자는 `LockSetGuard::write_permit`만 제공한다.
pub(crate) struct WritePermit<'guard> {
    guard: PermitGuard<'guard>,
    project_fingerprint: String,
    session_id: LockSessionId,
    provider_kind: LockProviderKind,
    targets: Vec<ProjectRelativePath>,
}

enum PermitGuard<'guard> {
    Borrowed(&'guard mut LockSetGuard),
    #[cfg(test)]
    Owned(Box<LockSetGuard>),
}

impl PermitGuard<'_> {
    fn get(&self) -> &LockSetGuard {
        match self {
            Self::Borrowed(guard) => guard,
            #[cfg(test)]
            Self::Owned(guard) => guard,
        }
    }

    fn get_mut(&mut self) -> &mut LockSetGuard {
        match self {
            Self::Borrowed(guard) => guard,
            #[cfg(test)]
            Self::Owned(guard) => guard,
        }
    }
}

impl<'guard> WritePermit<'guard> {
    pub(super) fn issue(guard: &'guard mut LockSetGuard) -> Result<Self, WritePermitError> {
        Self::issue_from(PermitGuard::Borrowed(guard))
    }

    #[cfg(test)]
    pub(super) fn issue_owned(guard: Box<LockSetGuard>) -> Result<Self, WritePermitError> {
        Self::issue_from(PermitGuard::Owned(guard))
    }

    fn issue_from(mut guard: PermitGuard<'guard>) -> Result<Self, WritePermitError> {
        if guard.get().state() != LockSetState::Active {
            return Err(WritePermitError::LockSetNotActive);
        }
        guard
            .get_mut()
            .validate_all()
            .map_err(WritePermitError::Validation)?;
        Ok(Self {
            project_fingerprint: guard.get().project_fingerprint().to_owned(),
            session_id: guard.get().session_id().clone(),
            provider_kind: guard.get().provider_kind(),
            targets: guard.get().targets().cloned().collect(),
            guard,
        })
    }

    pub(crate) fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    pub(crate) fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    pub(crate) fn provider_kind(&self) -> LockProviderKind {
        self.provider_kind
    }

    pub(crate) fn targets(&self) -> &[ProjectRelativePath] {
        &self.targets
    }

    pub(crate) fn validate_for(
        &mut self,
        project_fingerprint: &str,
        targets: &[ProjectRelativePath],
    ) -> Result<(), WritePermitError> {
        if self.guard.get().state() != LockSetState::Active {
            return Err(WritePermitError::LockSetNotActive);
        }
        if self.project_fingerprint != project_fingerprint {
            return Err(WritePermitError::ProjectMismatch);
        }
        if self.targets != targets {
            return Err(WritePermitError::TargetMismatch);
        }
        if self.guard.get().project_fingerprint() != self.project_fingerprint
            || self.guard.get().session_id() != &self.session_id
            || self.guard.get().provider_kind() != self.provider_kind
            || !self.guard.get().targets().eq(self.targets.iter())
        {
            return Err(WritePermitError::IdentityChanged);
        }
        self.guard
            .get_mut()
            .validate_all()
            .map_err(WritePermitError::Validation)?;
        if self.guard.get().state() != LockSetState::Active
            || self.guard.get().project_fingerprint() != self.project_fingerprint
            || self.guard.get().session_id() != &self.session_id
            || self.guard.get().provider_kind() != self.provider_kind
            || !self.guard.get().targets().eq(self.targets.iter())
        {
            return Err(WritePermitError::IdentityChanged);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum WritePermitError {
    LockSetNotActive,
    ProjectMismatch,
    TargetMismatch,
    IdentityChanged,
    Validation(LockError),
}

impl fmt::Display for WritePermitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::LockSetNotActive => "collaboration lock set is not active",
            Self::ProjectMismatch => "write permit project fingerprint differs",
            Self::TargetMismatch => "write permit target set differs",
            Self::IdentityChanged => "collaboration lock identity changed",
            Self::Validation(_) => "collaboration lock validation failed",
        })
    }
}

impl std::error::Error for WritePermitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Validation(source) => Some(source),
            _ => None,
        }
    }
}
