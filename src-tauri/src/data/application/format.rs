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
