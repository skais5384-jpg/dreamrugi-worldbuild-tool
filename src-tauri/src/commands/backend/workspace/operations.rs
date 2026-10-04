use super::*;
use crate::commands::workspace::DraftProblemProperty;
use crate::data::edit_recovery::model::{digest, Deposit};

fn mutation_problem(error: &artifact::template_mutation::TemplateMutationError) -> DraftProblem {
    use artifact::template_mutation::TemplateMutationErrorCategory as C;
    use artifact::{
        ArtifactChoiceValueLocation as Choice, ArtifactRichTextValueLocation as Rich,
        ArtifactScalarValueLocation as Scalar,
    };
    let property = if error.validation_scalar_location() == Some(Scalar::CurrentDefault)
        || error.validation_choice_location() == Some(Choice::CurrentDefault)
        || error.validation_rich_text_location() == Some(Rich::CurrentDefault)
        || matches!(
            error.category(),
            C::InvalidCurrentDefault
                | C::CurrentDefaultRepairRequired
                | C::InvalidCurrentDefaultRepair
        ) {
        DraftProblemProperty::Default
    } else {
        match error.category() {
            C::OptionNotFound
            | C::OptionAlreadyExists
            | C::OptionIsArchived
            | C::InvalidOptionDraft => DraftProblemProperty::Option,
            C::InvalidOptionOrder
            | C::InvalidFieldDraft
            | C::FieldIsNotChoice
            | C::ImmutableFieldChanged => DraftProblemProperty::Configuration,
            _ => DraftProblemProperty::Global,
        }
    };
    DraftProblem {
        category: format!("{:?}", error.category()),
        field: error.field_id().map(|id| id.to_string()),
        option: error.option_id().map(|id| id.to_string()),
        property,
    }
}

fn reply(entry: &Entry) -> Reply<Completed> {
    Ok(Completed::new(
        (),
        Ok(ResultDto::TemplateDraft {
            status: Box::new(entry.status()?),
        }),
    ))
}

pub(super) fn begin_owner(
    ctx: &mut Context,
    job: &Job,
    targets: Vec<crate::data::project_relative_path::ProjectRelativePath>,
    unpublished: bool,
) -> Reply<(
    Binding,
    crate::data::application::worker::SessionRegistration,
)> {
    if !unpublished {
        return begin(ctx, job, targets);
    }
    let registration = ctx
        .register_template_owner(job.provider.clone(), None)
        .map_err(|_| Code::SessionRejected)?;
    let binding = Binding {
        id: job.allocated,
        registration: registration.registration.clone(),
        key: None,
        views: vec![],
        draft_input: None,
        recovery: Arc::new(recovery::Observation::default()),
    };
    Ok((binding, registration))
}

pub(crate) fn execute(ctx: &mut Context, job: &Job) -> Reply<Completed> {
    if matches!(&*job.input, Work::RecoveryRestore { .. }) {
        return center::restore(ctx, job);
    }
    let mut registry = job.workspace.lock().map_err(|_| Code::Unavailable)?;
    if let Work::BeginTemplateDraft { project, view } = &*job.input {
        if registry.entries.len() >= 16 {
            return Err(Code::Full.into());
        }
        let draft_id: String = job.allocated.into();
        let base = view
            .map(|v| {
                job.views
                    .iter()
                    .find(|(id, _)| *id == v)
                    .map(|(_, v)| v.clone())
                    .ok_or(Code::WrongBinding)
            })
            .transpose()?;
        if let Some(v) = &base {
            let source = v.template()?;
            if registry.entries.values().any(|entry| {
                entry.project == *project
                    && entry
                        .artifact()
                        .is_ok_and(|artifact| artifact.template_id() == source.id)
            }) {
                return Err(Code::DuplicateConflict.into());
            }
            let loaded = ctx
                .read(|r| ArtifactRepository::new(r)?.load_template(source.id))
                .map_err(|_| Code::RuntimeRejected)?
                .map_err(|_| Code::RepositoryRejected)?;
            if loaded.source() != &source.token {
                return Err(Code::WrongBinding.into());
            }
            let time = timestamp()?;
            ctx.read(|ready| {
                let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                crate::data::repository::versions::ensure_baseline(
                    &repository,
                    ArtifactSourceId::Template(source.id),
                    &time,
                )
            })
            .map_err(|_| Code::RuntimeRejected)?
            .map_err(|_| Code::RepositoryRejected)?;
        }
        let seed = if base.is_none() {
            Some(
                artifact::template_mutation::whole::unpublished_seed(
                    convert::id(&draft_id)?,
                    timestamp()?,
                )
                .map_err(|_| Code::PreparationRejected)?,
            )
        } else {
            None
        };
        let artifact = match &base {
            Some(v) => v.template()?.id,
            None => seed.as_ref().ok_or(Code::WrongBinding)?.template_id(),
        };
        let target = ArtifactSourceId::Template(artifact)
            .path()
            .map_err(|_| Code::InvalidInput)?;
        let (binding, registration) = begin_owner(ctx, job, vec![target], base.is_none())?;
        let fingerprint = ctx
            .project_fingerprint()
            .ok_or(Code::RuntimeRejected)?
            .to_owned();
        let t = match &base {
            Some(v) => match &**v {
                View::Template(t) => t.artifact(),
                _ => return Err(Code::WrongBinding.into()),
            },
            None => seed.as_ref().ok_or(Code::WrongBinding)?,
        };
        let body = body_from_source(t)?;
        let mut entry = Entry {
            resume: None,
            comparison_checkpoint: None,
            residual: None,
            residual_ack: None,
            resumed_attempt: None,
            owner: binding.id,
            project: *project,
            fingerprint,
            draft_id,
            base,
            seed,
            body,
            generation: 1,
            saved_generation: view.map(|_| 1),
            snapshot: Id::new(),
            content: String::new(),
            phase: "editing",
            receipt: None,
            error: registration
                .original
                .as_ref()
                .err()
                .map(|_| Code::SessionRejected.into()),
            problems: vec![],
            outcome: None,
            submitted: None,
            deposit: None,
            original: Some(Box::new(registration)),
            expected_digest: None,
            restored_from: None,
            restore_assets: None,
            proof: None,
            release_first: None,
            release_latest: None,
            handoff_receipt: None,
            handoff_failure: None,
            discarded_input: None,
        };
        if entry.base.is_some() && entry.error.is_none() {
            let resumed = (|| -> Reply<()> {
                let id = ArtifactSourceId::Template(entry.artifact()?.template_id());
                let pending = ctx
                    .read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        crate::data::repository::drafts::latest_checkpoint(&repository, id)
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RecoveryRejected)?;
                let Some((mut envelope, checkpoint)) = pending else {
                    return Ok(());
                };
                // New field/option identities belong to the preserved input, not
                // the newly acquired editor owner. Compound document components
                // and repeated resume use this same deterministic allocation key.
                entry.draft_id = envelope.key.draft_id.clone();
                if envelope
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.unresolved())
                {
                    let target = id.path().map_err(|_| Code::WrongBinding)?;
                    if !ctx.can_recheck_latest_input(&binding.registration, &target) {
                        entry.phase = "uncertain";
                        entry.error = Some(Code::RecoveryRejected.into());
                        entry.original = Some(Box::new(envelope));
                        return Ok(());
                    }
                    envelope
                        .attempt
                        .as_mut()
                        .ok_or(Code::WrongBinding)?
                        .recovery_checked = true;
                }
                let plan = recovery::input_plan(&envelope, |input| {
                    let original = input
                        .originals
                        .iter()
                        .find(|original| {
                            original.kind
                                == crate::data::edit_recovery::model::OriginalKind::Template
                        })
                        .ok_or(Code::WrongBinding)?;
                    let original = artifact::decode_template(original.snapshot.as_bytes())
                        .map_err(|_| Code::RecoveryRejected)?;
                    let body =
                        TemplateBody::from_recovery(&input.draft).ok_or(Code::WrongBinding)?;
                    recovery_template::plan(&body, &original, entry.artifact()?)
                })?;
                if !plan.changes.is_empty() {
                    entry.resumed_attempt = envelope.attempt.clone();
                    entry.generation = envelope.key.generation.checked_add(1).ok_or(Code::Full)?;
                    entry.saved_generation = None;
                }
                if plan
                    .changes
                    .iter()
                    .any(|change| change.status == "conflict")
                {
                    entry.phase = "draft_conflict";
                    entry.error = Some(Code::OwnersRemain.into());
                    entry.resume = Some((envelope, plan));
                    entry.comparison_checkpoint = Some(checkpoint);
                    return Ok(());
                }
                if plan.changes.is_empty() {
                    let checkpoint = if envelope.residual.is_some() {
                        entry.generation =
                            envelope.key.generation.checked_add(1).ok_or(Code::Full)?;
                        entry.saved_generation = Some(entry.generation);
                        entry.resumed_attempt = envelope.attempt.clone();
                        entry.residual_ack = recovery::residual_ack(&envelope)?;
                        ctx.preserve_latest_input(entry.record(job, false)?.envelope)
                            .map_err(|_| Code::RecoveryRejected)?
                    } else {
                        checkpoint
                    };
                    let source = entry
                        .base
                        .as_ref()
                        .ok_or(Code::WrongBinding)?
                        .template()?
                        .token
                        .clone();
                    ctx.read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        crate::data::repository::drafts::clear_if_sources_match(
                            &repository,
                            &checkpoint,
                            &[&source],
                        )
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RecoveryRejected)?;
                    return Ok(());
                }
                entry.residual = recovery::remaining_input(&envelope, &plan)?;
                entry.residual_ack = recovery::residual_ack(&envelope)?;
                let choices = plan
                    .changes
                    .iter()
                    .filter(|change| change.status == "proposed")
                    .map(|change| change.id.clone())
                    .collect::<Vec<_>>();
                entry.body = serde_json::from_value(
                    plan.apply(&choices)
                        .map_err(|_| Code::PreparationRejected)?,
                )
                .map_err(|_| Code::SerializationFailed)?;
                entry.body.composing = false;
                entry.generation = envelope.key.generation.checked_add(1).ok_or(Code::Full)?;
                entry.saved_generation = None;
                ctx.preserve_latest_input(entry.record(job, false)?.envelope)
                    .map_err(|_| Code::RecoveryRejected)?;
                entry.original = Some(Box::new(envelope));
                Ok(())
            })();
            if let Err(error) = resumed {
                entry.error = Some(error);
                entry.phase = "resume_failed";
            }
        }
        entry.rebuild_content()?;
        let mut notes = vec![];
        let capture = (|| {
            let record = entry.record(job, false)?;
            let deposit = Deposit::freeze(record.envelope).map_err(|e| {
                super::super::document_workspace::observed(&mut notes, e, Code::RecoveryRejected)
            })?;
            recovery::capture_assets(ctx, job, &deposit, &mut notes)
        })();
        if let Err(error) = capture {
            entry.error = Some(error);
        }
        entry.original = Some(Box::new((entry.original.take(), notes)));
        let mut result = reply(&entry)?;
        result.binding = Some(binding);
        registry.entries.insert(entry.owner, entry);
        registry.owners.store(
            registry.entries.len() + registry.documents.len(),
            std::sync::atomic::Ordering::Release,
        );
        return Ok(result);
    }
    let owner = job.input.session().ok_or(Code::WrongBinding)?;
    let entry = registry.entries.get_mut(&owner).ok_or(Code::UnknownId)?;
    if Some(entry.project) != job.input.project() {
        return Err(Code::WrongBinding.into());
    }
    let latest = if let Work::TemplateDraft {
        generation,
        body,
        action: DraftAction::Save | DraftAction::Checkpoint,
        ..
    } = &*job.input
    {
        if matches!(
            entry.phase,
            "draft_conflict" | "resume_failed" | "uncertain" | "saved_read_required"
        ) {
            return reply(entry);
        }
        if generation
            .parse::<u64>()
            .is_ok_and(|next| next > entry.generation)
        {
            let previous = ctx
                .session_control()
                .deposit_whole_draft(&job.binding.as_ref().ok_or(Code::WrongBinding)?.registration);
            match previous {
                Ok(Ok(Some(receipt))) => entry.handoff_receipt = Some(receipt),
                Ok(Ok(None)) => (),
                original => {
                    entry.handoff_failure = Some(Box::new(original));
                    entry.error = Some(Code::RecoveryRejected.into());
                    return reply(entry);
                }
            }
        }
        entry.accept_body(generation, body)?;
        if entry.base.is_some() {
            let envelope = entry.record(job, false)?.envelope;
            Some(
                ctx.preserve_latest_input(envelope)
                    .map_err(|_| Code::RecoveryRejected)?,
            )
        } else {
            None
        }
    } else {
        None
    };
    match &*job.input {
        Work::TemplateDraft {
            action: DraftAction::Checkpoint,
            ..
        } => reply(entry),
        Work::TemplateDraftContent {
            snapshot, offset, ..
        } => {
            let content = chunk(entry.snapshot, &entry.content, *snapshot, offset)?;
            Ok(Completed::new(
                (),
                Ok(ResultDto::TemplateDraftContent { content }),
            ))
        }
        Work::ResumeTemplateDraft { selected, .. } => {
            if entry.phase != "draft_conflict" {
                return Err(Code::WrongBinding.into());
            }
            let (envelope, _) = entry.resume.as_ref().ok_or(Code::NoDraft)?;
            let envelope = envelope.clone();
            let id = entry.artifact()?.template_id();
            let current = ctx
                .read(|ready| ArtifactRepository::new(ready)?.load_template(id))
                .map_err(|_| Code::RuntimeRejected)?;
            let loaded = current.map_err(|_| Code::RepositoryRejected)?;
            let plan = recovery::input_plan(&envelope, |input| {
                let original = input
                    .originals
                    .iter()
                    .find(|original| {
                        original.kind == crate::data::edit_recovery::model::OriginalKind::Template
                    })
                    .ok_or(Code::WrongBinding)?;
                let original = artifact::decode_template(original.snapshot.as_bytes())
                    .map_err(|_| Code::RecoveryRejected)?;
                let body = TemplateBody::from_recovery(&input.draft).ok_or(Code::WrongBinding)?;
                recovery_template::plan(&body, &original, loaded.artifact())
            })?;
            if entry
                .base
                .as_ref()
                .ok_or(Code::WrongBinding)?
                .template()?
                .token
                != *loaded.source()
            {
                entry.body = body_from_source(loaded.artifact())?;
                entry.base = Some(Arc::new(View::Template(loaded)));
                entry.resume = Some((envelope, plan));
                entry.rebuild_content()?;
                return reply(entry);
            }
            entry.residual = recovery::remaining_input(&envelope, &plan)?;
            entry.residual_ack = recovery::residual_ack(&envelope)?;
            if selected.iter().any(|id| {
                !plan
                    .changes
                    .iter()
                    .any(|change| change.id == *id && change.status == "conflict")
            }) {
                return Err(Code::InvalidInput.into());
            }
            let mut choices = plan
                .changes
                .iter()
                .filter(|change| change.status == "proposed")
                .map(|change| change.id.clone())
                .collect::<Vec<_>>();
            choices.extend(selected.iter().cloned());
            entry.body = serde_json::from_value(
                plan.apply(&choices)
                    .map_err(|_| Code::PreparationRejected)?,
            )
            .map_err(|_| Code::SerializationFailed)?;
            entry.body.composing = false;
            entry.generation = envelope.key.generation.checked_add(1).ok_or(Code::Full)?;
            entry.saved_generation = None;
            entry.resume = None;
            entry.phase = "editing";
            entry.error = None;
            entry.rebuild_content()?;
            ctx.preserve_latest_input(entry.record(job, false)?.envelope)
                .map_err(|_| Code::RecoveryRejected)?;
            entry.original = Some(Box::new((entry.original.take(), envelope)));
            reply(entry)
        }
        Work::RefreshTemplateDraft { .. } => {
            refresh(ctx, entry)?;
            reply(entry)
        }
        Work::TemplateDraft {
            generation,
            body,
            action: DraftAction::Save,
            ..
        } => {
            entry.accept_body(generation, body)?;
            if matches!(
                entry.phase,
                "uncertain" | "saved_read_required" | "draft_conflict" | "resume_failed"
            ) {
                entry.error = Some(Code::OwnersRemain.into());
                return reply(entry);
            }
            if entry
                .submitted
                .as_ref()
                .is_some_and(|r| r.envelope.key.generation == entry.generation)
                || entry.deposit.is_some()
            {
                entry.error = Some(Code::DuplicateConflict.into());
                return reply(entry);
            }
            entry.error = None;
            entry.problems.clear();
            entry.outcome = None;
            if entry.body.composing {
                entry.error = Some(Code::InvalidInput.into());
                entry.problems.push(DraftProblem {
                    category: "CompositionActive".into(),
                    field: None,
                    option: None,
                    property: DraftProblemProperty::Global,
                });
                return reply(entry);
            }
            let draft = match normalize(entry) {
                Ok(d) => d,
                Err(e) => {
                    entry.error = Some(e);
                    return reply(entry);
                }
            };
            let mut record = entry.record(job, true)?;
            // 제출 전에 보관 가능성도 검증한다. 원문/오류 객체는 registry 밖으로 소비하지 않는다.
            if let Err(e) = Deposit::freeze(record.envelope.clone()) {
                entry.original = Some(Box::new(e));
                entry.error = Some(Code::RecoveryRejected.into());
                return reply(entry);
            }
            let frozen =
                Deposit::freeze(record.envelope.clone()).map_err(|_| Code::RecoveryRejected)?;
            let mut notes = vec![];
            let binding = job.binding.as_ref().ok_or(Code::WrongBinding)?;
            if ctx.session_key(&binding.registration).is_none() {
                let target = ArtifactSourceId::Template(entry.artifact()?.template_id())
                    .path()
                    .map_err(|_| Code::InvalidInput)?;
                let activation = ctx.activate_template_owner(&binding.registration, target);
                if !matches!(&activation, Ok(Ok(()))) {
                    entry.original = Some(Box::new(activation));
                    entry.error = Some(Code::SessionRejected.into());
                    return reply(entry);
                }
            }
            let key = ctx
                .session_key(&binding.registration)
                .ok_or(Code::WrongBinding)?;
            if let Some(receipt) = &entry.restore_assets {
                let copied = (|| -> Reply<()> {
                    if let Some(base) = &entry.base {
                        let source = base.template()?;
                        let current = ctx
                            .read(|ready| ArtifactRepository::new(ready)?.load_template(source.id))
                            .map_err(|_| Code::RuntimeRejected)?
                            .map_err(|_| Code::RepositoryRejected)?;
                        if current.source() != &source.token {
                            return Err(Code::WrongBinding.into());
                        }
                    }
                    let store = job.recovery.connect().map_err(|_| Code::SinkUnavailable)?;
                    let store = store.lock().map_err(|_| Code::Unavailable)?;
                    let deposit = store
                        .read(&receipt.key, &receipt.deposit_id)
                        .map_err(|_| Code::RecoveryRejected)?;
                    if deposit.payload_digest() != receipt.digest {
                        return Err(Code::WrongBinding.into());
                    }
                    let chosen = entry.body.recovery(
                        entry
                            .base
                            .as_ref()
                            .map(|base| base.template().map(|t| t.id.to_string()))
                            .transpose()?,
                    );
                    ctx.read(|ready| {
                        store.restore_selected_assets(
                            &deposit,
                            &chosen,
                            ready.locked_project().canonical_root(),
                        )
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RecoveryRejected)?;
                    Ok(())
                })();
                if let Err(error) = copied {
                    entry.error = Some(error);
                    return reply(entry);
                }
            }
            let captured = recovery::capture_assets(ctx, job, &frozen, &mut notes);
            entry.original = Some(Box::new((entry.original.take(), notes)));
            if let Err(error) = captured {
                entry.error = Some(error);
                return reply(entry);
            }
            recovery::connect(&mut ctx.session_control(), job, binding);
            let payload = entry.payload(job, &record);
            let previous = match ctx
                .session(&key)
                .map_err(|_| Code::SessionRejected)?
                .bind_whole_draft(payload, |old, new| {
                    entry.owns_payload(old)
                        && entry.owns_payload(new)
                        && old.recovery.envelope.key.generation
                            < new.recovery.envelope.key.generation
                        && old.recovery.envelope.originals == new.recovery.envelope.originals
                }) {
                Ok(previous) => previous,
                Err(e) => {
                    entry.original = Some(Box::new(e));
                    entry.error = Some(Code::OwnersRemain.into());
                    return reply(entry);
                }
            };
            if latest.is_some() {
                ctx.read(|ready| {
                    let repository =
                        ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                    crate::data::repository::drafts::preserve(&repository, record.envelope.clone())
                })
                .map_err(|_| Code::RuntimeRejected)?
                .map_err(|_| Code::RecoveryRejected)?;
            }
            let (diagnostic, candidate, original): (_, _, Box<dyn Any + Send>) =
                if let Some(base) = &entry.base {
                    let input = templates::whole::WholeTemplateInput {
                        source: base.template()?,
                        timestamp_utc: timestamp()?,
                        draft,
                    };
                    let (execution, resolved) = ctx
                        .session(&key)
                        .map_err(|_| Code::SessionRejected)?
                        .write_whole_template(&input);
                    let diagnostic = execution
                        .as_ref()
                        .ok()
                        .and_then(|d| d.original.as_ref().ok())
                        .map(|e| e.execution.diagnostic());
                    let candidate = execution
                        .as_ref()
                        .ok()
                        .and_then(|d| d.original.as_ref().ok())
                        .and_then(|e| e.candidate())
                        .cloned();
                    if let Some(error) = execution
                        .as_ref()
                        .ok()
                        .and_then(|d| d.original.as_ref().ok())
                        .and_then(|e| e.rejection())
                        .and_then(|e| e.domain_cause())
                    {
                        match error {
                            templates::TemplateUseCaseError::Mutation(e) => {
                                entry.problems.push(mutation_problem(e))
                            }
                            templates::TemplateUseCaseError::SourceMismatch
                            | templates::TemplateUseCaseError::RevisionMismatch => {
                                entry.phase = "conflict"
                            }
                            _ => (),
                        }
                    }
                    (
                        diagnostic,
                        candidate,
                        Box::new((input, execution, resolved)),
                    )
                } else {
                    let prepared = ctx.prepare_whole_template(
                        entry.seed.as_ref().ok_or(Code::WrongBinding)?,
                        &timestamp()?,
                        draft,
                    );
                    match prepared {
                        Ok(mut prepared) => {
                            let (execution, resolved) = ctx
                                .session(&key)
                                .map_err(|_| Code::SessionRejected)?
                                .create_whole_template(&mut prepared);
                            (
                                execution.as_ref().ok().map(|d| d.original.diagnostic()),
                                Some(prepared.candidate().clone()),
                                Box::new((prepared, execution, resolved)),
                            )
                        }
                        Err(e) => {
                            if let templates::TemplatePreparationError::Domain(
                                templates::TemplateUseCaseError::Mutation(problem),
                            ) = &e
                            {
                                entry.problems.push(mutation_problem(problem));
                            }
                            entry.error = Some(Code::PreparationRejected.into());
                            (None, None, Box::new(e))
                        }
                    }
                };
            let mut attempt = recovery::attempt_ref(job.operation, diagnostic.as_ref());
            attempt.submitted_generation = entry.generation;
            if let Some(candidate) = &candidate {
                match artifact::encode_template(candidate) {
                    Ok(bytes) => attempt.candidate_digest = Some(digest(&bytes)),
                    Err(e) => {
                        entry.error = Some(Code::SerializationFailed.into());
                        entry.original = Some(Box::new((original, e)));
                        entry.phase = "uncertain";
                        record
                            .attempt
                            .set(attempt.clone())
                            .map_err(|_| Code::DuplicateConflict)?;
                        record.envelope.attempt = Some(attempt);
                        entry.submitted = Some(record);
                        return reply(entry);
                    }
                }
            }
            record
                .attempt
                .set(attempt.clone())
                .map_err(|_| Code::DuplicateConflict)?;
            record.envelope.attempt = Some(attempt);
            entry.original = Some(Box::new((previous, original)));
            entry.submitted = Some(record.clone());
            let completed_checkpoint = if latest.is_some() {
                ctx.preserve_latest_input(record.envelope.clone())
                    .map(Some)
                    .map_err(|_| Code::RecoveryRejected.into())
            } else {
                Ok(None)
            };
            if let Some(d) = diagnostic {
                let disk = d.disk;
                entry.outcome = Some(Box::new(write(
                    entry.owner,
                    Some(entry.artifact()?.template_id().to_string()),
                    d,
                    Some(candidate.is_some()),
                    vec![],
                )));
                match disk {
                    DiskState::Committed => {
                        entry.saved_generation = Some(entry.generation);
                        entry.expected_digest = entry
                            .submitted
                            .as_ref()
                            .and_then(|r| r.envelope.attempt.as_ref())
                            .and_then(|a| a.candidate_digest.clone());
                        entry.phase = "saved_read_required";
                        if let Err(error) = refresh(ctx, entry) {
                            entry.error = Some(error);
                        }
                    }
                    DiskState::NoWrite => {
                        entry.saved_generation = Some(entry.generation);
                        entry.phase = "saved";
                    }
                    DiskState::Uncertain => {
                        entry.phase = "uncertain";
                        entry.error = Some(Code::RecoveryRejected.into());
                    }
                    _ => {
                        entry.error = Some(Code::SaveRejected.into());
                    }
                }
            } else {
                entry.error = Some(Code::SaveRejected.into());
            }
            let completed_checkpoint = match completed_checkpoint {
                Ok(checkpoint) => checkpoint,
                Err(error) => {
                    entry.error = Some(error);
                    if entry.phase == "saved" {
                        entry.phase = "saved_read_required";
                    }
                    return reply(entry);
                }
            };
            if entry.phase == "saved" {
                if let Some(completed_checkpoint) = completed_checkpoint {
                    ctx.read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        crate::data::repository::drafts::clear(&repository, &completed_checkpoint)
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RecoveryRejected)?;
                }
            }
            reply(entry)
        }
        _ => Err(Code::InvalidInput.into()),
    }
}

fn refresh(ctx: &mut Context, entry: &mut Entry) -> Reply<()> {
    if entry.phase != "saved_read_required" {
        return Err(Code::InvalidInput.into());
    }
    let expected = entry.expected_digest.as_ref().ok_or(Code::WrongBinding)?;
    let id = entry.artifact()?.template_id();
    let loaded = ctx
        .read(|r| ArtifactRepository::new(r)?.load_template(id))
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    let digest: String = loaded
        .source()
        .sha256()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if &digest != expected {
        return Err(Code::WrongBinding.into());
    }
    fn consume(fields: &mut [DraftField]) {
        for field in fields {
            field.restore = false;
            field.archive_index = None;
            field.archive_order.clear();
            field.archive_title = None;
            match &mut field.configuration {
                DraftConfiguration::Group { members, .. } => consume(members),
                DraftConfiguration::SingleChoice { options }
                | DraftConfiguration::MultiChoice { options } => {
                    for option in options {
                        option.restore = false;
                        option.archive_index = None;
                        option.archive_order.clear();
                    }
                }
                _ => (),
            }
        }
    }
    consume(&mut entry.body.fields);
    entry.base = Some(Arc::new(View::Template(loaded)));
    entry.seed = None;
    entry.phase = "saved";
    entry.error = None;
    entry.rebuild_content()
}

pub(crate) fn control(
    ctx: &mut CleanupContext<'_, PendingEdit, Receipt>,
    job: &Job,
) -> Reply<Completed> {
    let mut registry = job.workspace.lock().map_err(|_| Code::Unavailable)?;
    let owner = job.input.session().ok_or(Code::WrongBinding)?;
    let entry = registry.entries.get_mut(&owner).ok_or(Code::UnknownId)?;
    if Some(entry.project) != job.input.project() {
        return Err(Code::WrongBinding.into());
    }
    match &*job.input {
        Work::TemplateDraft {
            generation,
            body,
            action: DraftAction::Deposit,
            ..
        } => {
            entry.accept_body(generation, body)?;
            let binding = job.binding.as_ref().ok_or(Code::WrongBinding)?;
            if ctx
                .observe_session(&binding.registration)
                .map_err(|_| Code::SessionRejected)?
                .0
                .session_id()
                .is_some()
            {
                recovery::connect(ctx, job, binding);
            }
            // 먼저 worker의 실제 이전 P를 인수한다. gen8의 새 proof로 gen7 P를 해제하지 않는다.
            match ctx.deposit_whole_draft(&binding.registration) {
                Ok(Ok(Some(receipt))) => entry.handoff_receipt = Some(receipt),
                Ok(Ok(None)) => (),
                original => {
                    entry.handoff_failure = Some(Box::new(original));
                    entry.error = Some(Code::RecoveryRejected.into());
                    return reply(entry);
                }
            }
            if entry.deposit.is_none() {
                entry.deposit = Some(match &entry.submitted {
                    Some(r) if r.envelope.key.generation == entry.generation => r.clone(),
                    _ => entry.record(job, false)?,
                });
            }
            let record = entry.deposit.as_ref().ok_or(Code::NoDraft)?;
            let result = (|| {
                let deposit = Deposit::freeze(record.envelope.clone()).map_err(Arc::new)?;
                let proof =
                    recovery::accept_latest_or_legacy(ctx, job, &deposit).map_err(Arc::new)?;
                if !proof.matches(
                    deposit.key(),
                    &deposit.envelope().deposit_id,
                    deposit.payload_digest(),
                ) {
                    return Err(Arc::new(
                        crate::data::edit_recovery::error::RecoveryError::new(
                            crate::data::edit_recovery::error::Category::DigestMismatch,
                            crate::data::edit_recovery::error::Stage::Revalidate,
                        ),
                    ));
                }
                Ok((
                    ReceiptDto {
                        key: deposit.key().clone(),
                        deposit_id: deposit.envelope().deposit_id.clone(),
                        digest: deposit.payload_digest().into(),
                    },
                    proof,
                ))
            })();
            match result {
                Ok((receipt, proof)) => {
                    entry.receipt = Some(receipt);
                    entry.error = None;
                    entry.proof = Some(proof);
                    if let Some(b) = &job.binding {
                        b.recovery.record(None);
                    }
                }
                Err(e) => {
                    if let Some(b) = &job.binding {
                        b.recovery.record(Some(e.clone()));
                    }
                    entry.error = Some(Code::RecoveryRejected.into());
                }
            }
            reply(entry)
        }
        Work::ReleaseTemplateDraft {
            generation,
            body,
            discard,
            ..
        } => {
            let cancel_comparison = entry.phase == "draft_conflict"
                && entry.resume.is_some()
                && entry.submitted.is_none()
                && entry.expected_digest.is_none()
                && entry.saved_generation.is_none();
            let _comparison_custody = if cancel_comparison {
                if *discard
                    || parse_generation(generation)? != entry.generation
                    || body.as_ref().is_some_and(|body| body != &entry.body)
                {
                    return Err(Code::WrongBinding.into());
                }
                let checkpoint = entry
                    .comparison_checkpoint
                    .as_ref()
                    .ok_or(Code::NoReceipt)?;
                let durable = ctx
                    .hold_comparison_input(
                        &job.binding.as_ref().ok_or(Code::WrongBinding)?.registration,
                        checkpoint,
                    )
                    .map_err(|_| Code::RecoveryRejected)?;
                Some(durable)
            } else {
                None
            };
            if let Some(body) = body {
                entry.accept_body(generation, body)?;
            }
            if parse_generation(generation)? != entry.generation {
                return Err(Code::WrongBinding.into());
            }
            if !cancel_comparison
                && !discard
                && entry.saved_generation != Some(entry.generation)
                && !entry.receipt.as_ref().is_some_and(|r| {
                    r.key.generation == entry.generation && r.key.draft_id == entry.draft_id
                })
            {
                return Err(Code::NoReceipt.into());
            }
            // 불확정 쓰기는 UI 포기와 별개로 기존 runtime 복구/세션 해제가 계속 책임진다.
            let binding = job.binding.as_ref().ok_or(Code::WrongBinding)?;
            if *discard {
                entry.discarded_input = ctx
                    .discard_whole_active(&binding.registration, |p| entry.owns_payload(p))
                    .map_err(|_| Code::OwnersRemain)?;
            }
            let (snapshot, active) = ctx
                .observe_session(&binding.registration)
                .map_err(|_| Code::SessionRejected)?;
            if active
                || matches!(
                    format!("{:?}", snapshot.state()).as_str(),
                    "Saving" | "Recovering"
                )
            {
                return Err(Code::OwnersRemain.into());
            }
            if snapshot.state() != crate::data::edit_session::EditSessionState::ReadOnly {
                let release = if snapshot.state()
                    == crate::data::edit_session::EditSessionState::ReleaseFailed
                {
                    ctx.retry_session_release(&binding.registration)
                } else {
                    ctx.end_session_observed(&binding.registration)
                }
                .map_err(|_| Code::ReleaseRejected)?;
                if release.original.is_err() {
                    entry.error = Some(Code::ReleaseRejected.into());
                    if entry.release_first.is_none() {
                        entry.release_first = Some(Box::new(release));
                    } else {
                        entry.release_latest = Some(Box::new(release));
                    }
                    return reply(entry);
                }
            }
            if !discard && entry.saved_generation == Some(entry.generation) {
                if let Some(source) = &entry.base {
                    let source = source.template()?;
                    let expected = source
                        .token
                        .sha256()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>();
                    let time = timestamp()?;
                    ctx.confirm_content_version(
                        &binding.registration,
                        ArtifactSourceId::Template(source.id),
                        &time,
                        &expected,
                    )
                    .map_err(|error| {
                        entry.original = Some(Box::new(error));
                        Code::RepositoryRejected
                    })?;
                }
            }
            ctx.remove_session(&binding.registration)
                .map_err(|_| Code::ReleaseRejected)?;
            let original = registry.entries.remove(&owner).ok_or(Code::UnknownId)?;
            registry.owners.store(
                registry.entries.len() + registry.documents.len(),
                std::sync::atomic::Ordering::Release,
            );
            let mut result = Completed::new(original, Ok(ResultDto::Control { error: None }));
            result.removed_binding = Some(owner);
            Ok(result)
        }
        _ => Err(Code::InvalidInput.into()),
    }
}

pub(super) fn chunk(snapshot: Id, text: &str, requested: Id, offset: &str) -> Reply<ContentChunk> {
    let start = offset.parse::<usize>().map_err(|_| Code::InvalidInput)?;
    if snapshot != requested
        || start.to_string() != offset
        || start > text.len()
        || !text.is_char_boundary(start)
    {
        return Err(Code::WrongBinding.into());
    }
    let mut end = (start + 65536).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(ContentChunk {
        snapshot,
        offset: offset.into(),
        next: (end < text.len()).then(|| end.to_string()),
        text: text[start..end].into(),
    })
}
