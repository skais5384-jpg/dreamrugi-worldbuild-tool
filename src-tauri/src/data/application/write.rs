use std::{convert::Infallible, fmt};

use super::diagnostics::{
    ApplicationCategory, ApplicationDiagnostic, ApplicationError, BuildError,
};
use crate::data::{
    collaboration_lock::LockSessionId,
    edit_session::{
        EditSessionService, EditSessionSnapshot, EditSessionState, WriteOperationOutcome,
    },
    project_relative_path::ProjectRelativePath,
    project_runtime::{ProjectRuntime, RuntimeSnapshot},
    repository::{
        ArtifactCommitError, ArtifactCommitOutcome, ArtifactRepository, ArtifactSourceId,
        ArtifactWriteError, CanonicalWritePlan,
    },
};

/// 정규 경로의 유일한 선언이다. 생성 후에는 순서·대상을 바꾸거나 raw path로 조립할 수 없다.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ExactWriteTargets {
    paths: Vec<ProjectRelativePath>,
}

impl ExactWriteTargets {
    pub(super) fn new(
        ids: impl IntoIterator<Item = ArtifactSourceId>,
    ) -> Result<Self, ApplicationError> {
        let mut paths = ids
            .into_iter()
            .map(|id| {
                id.path()
                    .map_err(|_| ApplicationError::closed(ApplicationCategory::InvalidTarget))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if paths.is_empty() {
            return Err(ApplicationError::closed(ApplicationCategory::EmptyTargets));
        }
        paths.sort();
        if paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ApplicationError::closed(
                ApplicationCategory::DuplicateTarget,
            ));
        }
        Ok(Self { paths })
    }

    /// 후속 begin_edit도 이 동일 집합을 복사해서 사용한다. executor는 세션을 시작/변경하지 않는다.
    pub(super) fn session_targets(&self) -> &[ProjectRelativePath] {
        &self.paths
    }

    fn matches(&self, plan: &CanonicalWritePlan) -> bool {
        // G4가 실제 소유한 목록을 비교할 뿐, plan의 권한이나 내용을 덮어쓰지 않는다.
        let mut actual: Vec<_> = plan.targets().collect();
        actual.sort();
        actual.into_iter().eq(self.paths.iter())
    }
}

pub(super) struct WriteRequest<'a> {
    pub(super) project: &'a str,
    pub(crate) session: &'a LockSessionId,
    pub(super) targets: &'a ExactWriteTargets,
}

/// value에는 candidate/warnings 등 업무 결과 owner를 둘 수 있다. 실패해도 executor가 버리지 않는다.
pub(super) enum WriteDecision<T> {
    Write { plan: CanonicalWritePlan, value: T },
    NoWrite(T),
}

pub(crate) enum TransactionResult {
    PlanMismatch,
    PrepareFailed(ArtifactWriteError),
    Commit(Result<ArtifactCommitOutcome, ArtifactCommitError>),
}

pub(crate) enum BodyOutcome<T, E> {
    Rejected(BuildError<E>),
    NoWrite(T),
    Write {
        value: T,
        transaction: TransactionResult,
    },
}

/// session wrapper를 통째로 보유해 disk 결과와 보조 permit 원인이 함께 살아 있게 한다.
#[must_use = "disk 결과, recovery 필요와 session 상태를 함께 확인해야 한다"]
pub(crate) struct WriteExecution<T, E> {
    pub(crate) result:
        Result<WriteOperationOutcome<BodyOutcome<T, E>, Infallible>, ApplicationError>,
    pub(crate) runtime: RuntimeSnapshot,
    pub(crate) session: EditSessionSnapshot,
}

impl<T, E> WriteExecution<T, E> {
    pub(crate) fn body(&self) -> Option<&BodyOutcome<T, E>> {
        self.result
            .as_ref()
            .ok()
            .map(|outcome| match outcome.operation_result() {
                Ok(body) => body,
                Err(never) => match *never {},
            })
    }
    pub(crate) fn diagnostic(&self) -> ApplicationDiagnostic {
        ApplicationDiagnostic::from_execution(self)
    }
    pub(crate) fn value(&self) -> Option<&T> {
        match self.body()? {
            BodyOutcome::Rejected(_) => None,
            BodyOutcome::NoWrite(value) | BodyOutcome::Write { value, .. } => Some(value),
        }
    }
}
impl<T, E> fmt::Debug for WriteExecution<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WriteExecution")
            .field(&self.diagnostic())
            .finish()
    }
}
impl<T, E> fmt::Debug for BodyOutcome<T, E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rejected(_) => "BodyOutcome::Rejected([private cause])",
            Self::NoWrite(_) => "BodyOutcome::NoWrite([opaque owner])",
            Self::Write { .. } => "BodyOutcome::Write([opaque owner and transaction])",
        })
    }
}

/// runtime/session과 입력/builder를 빌린 동기 경계다. 거부 시 실행하지 않은 closure owner도 보존된다.
/// builder에는 repository만 빌려주며 permit/Ready/prepared를 제공하지 않는다.
pub(super) fn execute_write_operation<P, R, I, T, E, F>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    request: WriteRequest<'_>,
    input: &mut I,
    build: &mut F,
) -> WriteExecution<T, E>
where
    F: FnMut(&ArtifactRepository<'_, '_>, &mut I) -> Result<WriteDecision<T>, BuildError<E>>,
{
    let result = run(runtime, session, request, input, build);
    // 사후 처리는 바깥 ?보다 먼저, 모든 ready/repository/permit borrow가 끝난 뒤에 한다.
    let mut execution = WriteExecution {
        result,
        runtime: runtime.snapshot(),
        session: session.snapshot(),
    };
    if execution.diagnostic().recovery_required {
        runtime.invalidate_recovery();
        execution.runtime = runtime.snapshot();
    }
    execution
}

fn run<P, R, I, T, E, F>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    request: WriteRequest<'_>,
    input: &mut I,
    build: &mut F,
) -> Result<WriteOperationOutcome<BodyOutcome<T, E>, Infallible>, ApplicationError>
where
    F: FnMut(&ArtifactRepository<'_, '_>, &mut I) -> Result<WriteDecision<T>, BuildError<E>>,
{
    let snapshot = session.snapshot();
    if runtime.project_fingerprint() != request.project
        || snapshot.project_fingerprint() != Some(request.project)
    {
        return Err(ApplicationError::closed(
            ApplicationCategory::ProjectMismatch,
        ));
    }
    if snapshot.session_id() != Some(request.session) {
        return Err(ApplicationError::closed(
            ApplicationCategory::SessionMismatch,
        ));
    }
    if snapshot.targets() != request.targets.session_targets() {
        return Err(ApplicationError::closed(
            ApplicationCategory::SessionTargetsMismatch,
        ));
    }
    if snapshot.state() != EditSessionState::Editing {
        return Err(ApplicationError::closed(
            ApplicationCategory::InvalidSessionState,
        ));
    }
    let ready = runtime.ready().map_err(ApplicationError::runtime)?;
    session
        .run_validated_write(
            request.project,
            request.targets.session_targets(),
            |mut permit| {
                let repository = match ArtifactRepository::new(&ready) {
                    Ok(repository) => repository,
                    Err(error) => return Ok(BodyOutcome::Rejected(error.into())),
                };
                let decision = match build(&repository, input) {
                    Ok(decision) => decision,
                    Err(error) => return Ok(BodyOutcome::Rejected(error)),
                };
                let (plan, value) = match decision {
                    WriteDecision::NoWrite(value) => return Ok(BodyOutcome::NoWrite(value)),
                    WriteDecision::Write { plan, value } => (plan, value),
                };
                let transaction = if !request.targets.matches(&plan) {
                    TransactionResult::PlanMismatch
                } else {
                    // 전체 candidate encode와 exact 검사가 끝나야 한 번만 prepare하고 즉시 commit한다.
                    match plan.prepare_create_namespaces(&repository, &mut permit) {
                        Err(error) => TransactionResult::PrepareFailed(error),
                        Ok(namespace_guard) => {
                            let result = match plan.prepare(&repository, permit) {
                                Ok(prepared) => TransactionResult::Commit(prepared.commit()),
                                Err(error) => TransactionResult::PrepareFailed(error),
                            };
                            drop(namespace_guard);
                            result
                        }
                    }
                };
                Ok(BodyOutcome::Write { value, transaction })
            },
        )
        .map_err(ApplicationError::session)
}
