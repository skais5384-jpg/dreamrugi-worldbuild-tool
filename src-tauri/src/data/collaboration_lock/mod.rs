mod coordinator;
mod model;
mod permit;
mod service;

#[cfg(test)]
mod tests;

#[allow(
    unused_imports,
    reason = "M1-5F 기반 API 일부는 후속 session/SVN 결합 전까지 production 호출자가 없다"
)]
pub(crate) use coordinator::{
    LockCoordinator, LockCoordinatorError, LockRequestValidationError, LockSetGuard,
    LockSetReleaseError, LockSetState, PartialAcquireCleanup, ValidatedLockSetRequest,
};
#[allow(
    unused_imports,
    reason = "M1-5F 기반 API 일부는 후속 session/SVN 결합 전까지 production 호출자가 없다"
)]
pub(crate) use model::{
    LockAcquireRequest, LockCapabilities, LockError, LockErrorCategory, LockOperation,
    LockProviderInfo, LockProviderKind, LockSessionId, LockSetIdentityError, SafeOwnerIdentity,
};
pub(crate) use permit::{WritePermit, WritePermitError};
#[allow(
    unused_imports,
    reason = "M1-5F 기반 API 일부는 후속 session/SVN 결합 전까지 production 호출자가 없다"
)]
pub(crate) use service::{HeldLock, HeldLockState, LockService, NoLockService};
