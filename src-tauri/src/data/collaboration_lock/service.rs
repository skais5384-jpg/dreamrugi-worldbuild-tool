use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::project_relative_path::ProjectRelativePath;
use super::model::{
    LockAcquireRequest, LockCapabilities, LockError, LockErrorCategory, LockOperation,
    LockProviderInfo, LockProviderKind, LockSessionId,
};

static PROVIDER_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldLockState {
    Active,
    Released,
}

/// provider별 token은 구현 내부 handle에만 둔다. 일반 transaction과 UI에는 노출하지 않는다.
pub(crate) trait HeldLock: Send {
    fn provider_kind(&self) -> LockProviderKind;
    fn provider_instance_id(&self) -> u64;
    fn project_fingerprint(&self) -> &str;
    fn session_id(&self) -> &LockSessionId;
    fn target(&self) -> &ProjectRelativePath;
    fn state(&self) -> HeldLockState;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// `provider_info`는 provider 인스턴스 생명주기 동안 변하지 않는 종류와 capability만
/// 반환하는 비차단·무부작용 metadata 조회다. owner, 인증, 네트워크와 현재 lock 상태는
/// 여기에 넣지 않고 `validate` 또는 향후 provider 전용 API에서 조회한다.
/// acquire/validate/release는 blocking일 수 있으므로 향후 Tauri 계층은 UI thread가
/// 아닌 blocking worker에서 실행해야 한다.
pub(crate) trait LockService: Send + Sync {
    fn provider_info(&self) -> LockProviderInfo;
    /// Admission is evaluated on the worker before any canonical operation.
    fn authorize_project_operation(&self) -> Result<(), String> {
        Ok(())
    }
    /// Fail closed before writing a new collaborative attachment ID.
    fn authorize_new_asset(&self, _id: &str) -> Result<(), String> {
        Err("svn_invalid_target".into())
    }
    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError>;
    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError>;
    /// 실패 시 handle을 소비하지 않으므로 호출자가 같은 잠금의 release를 재시도할 수 있다.
    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError>;
}

pub(crate) struct NoLockService {
    instance_id: u64,
}

impl NoLockService {
    pub(crate) fn new() -> Self {
        Self {
            instance_id: PROVIDER_INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn checked_handle<'a>(
        &self,
        held: &'a mut dyn HeldLock,
        operation: LockOperation,
    ) -> Result<&'a mut NoLockHeld, LockError> {
        if held.provider_kind() != LockProviderKind::None
            || held.provider_instance_id() != self.instance_id
        {
            return Err(LockError::for_held(
                LockErrorCategory::HeldLockProviderMismatch,
                LockProviderKind::None,
                operation,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            ));
        }
        let project = held.project_fingerprint().to_owned();
        let session = held.session_id().clone();
        let target = held.target().clone();
        held.as_any_mut()
            .downcast_mut::<NoLockHeld>()
            .ok_or_else(|| {
                LockError::for_held(
                    LockErrorCategory::HeldLockProviderMismatch,
                    LockProviderKind::None,
                    operation,
                    &project,
                    &session,
                    &target,
                )
            })
    }
}

impl Default for NoLockService {
    fn default() -> Self {
        Self::new()
    }
}

impl LockService for NoLockService {
    fn provider_info(&self) -> LockProviderInfo {
        LockProviderInfo {
            kind: LockProviderKind::None,
            capabilities: LockCapabilities {
                distributed: false,
                validation: true,
                owner_diagnostics: false,
                steal: false,
            },
        }
    }

    fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        Ok(Box::new(NoLockHeld {
            instance_id: self.instance_id,
            project_fingerprint: request.project_fingerprint().to_owned(),
            session_id: request.session_id().clone(),
            target: request.target().clone(),
            state: HeldLockState::Active,
        }))
    }

    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked_handle(held, LockOperation::Validate)?;
        if held.state == HeldLockState::Released {
            return Err(held.error(
                LockErrorCategory::HeldLockAlreadyReleased,
                LockOperation::Validate,
            ));
        }
        Ok(())
    }

    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let held = self.checked_handle(held, LockOperation::Release)?;
        if held.state == HeldLockState::Released {
            return Err(held.error(
                LockErrorCategory::HeldLockAlreadyReleased,
                LockOperation::Release,
            ));
        }
        held.state = HeldLockState::Released;
        Ok(())
    }
}

struct NoLockHeld {
    instance_id: u64,
    project_fingerprint: String,
    session_id: LockSessionId,
    target: ProjectRelativePath,
    state: HeldLockState,
}

impl NoLockHeld {
    fn error(&self, category: LockErrorCategory, operation: LockOperation) -> LockError {
        LockError::for_held(
            category,
            LockProviderKind::None,
            operation,
            &self.project_fingerprint,
            &self.session_id,
            &self.target,
        )
    }
}

impl HeldLock for NoLockHeld {
    fn provider_kind(&self) -> LockProviderKind {
        LockProviderKind::None
    }

    fn provider_instance_id(&self) -> u64 {
        self.instance_id
    }

    fn project_fingerprint(&self) -> &str {
        &self.project_fingerprint
    }

    fn session_id(&self) -> &LockSessionId {
        &self.session_id
    }

    fn target(&self) -> &ProjectRelativePath {
        &self.target
    }

    fn state(&self) -> HeldLockState {
        self.state
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
