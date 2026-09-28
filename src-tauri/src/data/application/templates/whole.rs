//! Template 단독 전체 저장도 기존 source/permit/transaction 경계로 한 번만 들어간다.
use super::*;
use artifact::template_mutation::whole::{self, TemplateDraftInput};

pub(crate) struct WholeTemplateInput {
    pub(crate) source: TemplateSource,
    pub(crate) timestamp_utc: String,
    pub(crate) draft: TemplateDraftInput,
}

pub(crate) fn update<P, R>(
    runtime: &mut ProjectRuntime,
    session: &mut EditSessionService<P, R>,
    context: TemplateWriteContext<'_>,
    input: &WholeTemplateInput,
) -> Result<TemplateMutationExecution, ApplicationError> {
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(input.source.id)])?;
    let mut candidate = None;
    let execution = execute_write_operation(
        runtime,
        session,
        WriteRequest {
            project: context.project,
            session: context.session,
            targets: &targets,
        },
        &mut candidate,
        &mut |repository, candidate| {
            let loaded = checked_source(repository, &input.source)?;
            let outcome = whole::prepare_template_draft(
                loaded.artifact(),
                input.source.expected_revision,
                &input.timestamp_utc,
                input.draft.clone(),
            )
            .map_err(|e| BuildError::domain(TemplateUseCaseError::Mutation(e)))?;
            match outcome {
                TemplateMutationOutcome::Unchanged => Ok(WriteDecision::NoWrite(())),
                TemplateMutationOutcome::Changed(changed) => {
                    let changed = candidate.insert(changed);
                    let plan =
                        CanonicalWritePlan::new().replace_template(changed, loaded.source())?;
                    Ok(WriteDecision::Write { plan, value: () })
                }
            }
        },
    );
    Ok(TemplateMutationExecution {
        execution,
        candidate,
    })
}

pub(crate) fn prepare_create(
    runtime: &mut ProjectRuntime,
    seed: &TemplateArtifact,
    timestamp: &str,
    draft: TemplateDraftInput,
) -> Result<PreparedTemplateCreate, TemplatePreparationError> {
    let _ready = runtime.ready().map_err(TemplatePreparationError::Runtime)?;
    let candidate = whole::prepare_new_template_draft(seed, timestamp, draft)
        .map_err(|e| TemplatePreparationError::Domain(TemplateUseCaseError::Mutation(e)))?;
    ticket(runtime.project_fingerprint().to_owned(), candidate, None)
}
