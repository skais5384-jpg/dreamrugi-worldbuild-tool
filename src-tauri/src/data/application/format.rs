//! 형식 변경도 일반 저장과 같은 exact target/session/source/transaction gate를 통과한다.
use super::{
    diagnostics::ApplicationError,
    templates::TemplateWriteContext,
    write::{
        execute_write_operation, ExactWriteTargets, WriteDecision, WriteExecution, WriteRequest,
    },
};
use crate::data::{
    edit_session::EditSessionService,
    project_runtime::ProjectRuntime,
    repository::{format, ArtifactSourceId},
};
pub(crate) fn execute_policy_batch<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    prepared: &crate::data::edit_recovery::policy_transition::Prepared,
) -> Result<WriteExecution<(), ()>, ApplicationError> {
    let sources = &prepared.sources;
    let targets = ExactWriteTargets::new(sources.iter().map(|(id, _)| *id))?;
    Ok(execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut (),
        &mut |repo, _| {
            format::validate_policy_backup(prepared)?;
            Ok(WriteDecision::Write {
                plan: format::build_policy_batch(repo, sources)?,
                value: (),
            })
        },
    ))
}
pub(crate) fn execute<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    id: ArtifactSourceId,
    source: &str,
    restore: Option<&str>,
) -> Result<WriteExecution<(), ()>, ApplicationError> {
    let targets = ExactWriteTargets::new([id])?;
    Ok(execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut (),
        &mut |repo, _| {
            Ok(WriteDecision::Write {
                plan: format::build(repo, id, source, restore)?,
                value: (),
            })
        },
    ))
}

/// Content restore shares ordinary exact-target transaction and lock gates.
pub(crate) fn restore_version<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    id: ArtifactSourceId,
    version: u64,
    source: &str,
    timestamp: &str,
) -> Result<WriteExecution<(), ()>, ApplicationError> {
    let targets = ExactWriteTargets::new([id])?;
    Ok(execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut (),
        &mut |repository, _| match crate::data::repository::versions::restore_plan(
            repository, id, version, source, timestamp,
        )? {
            Some(plan) => Ok(WriteDecision::Write { plan, value: () }),
            None => Ok(WriteDecision::NoWrite(())),
        },
    ))
}
