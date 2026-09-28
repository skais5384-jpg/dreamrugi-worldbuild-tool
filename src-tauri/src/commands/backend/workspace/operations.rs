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
            let loaded = ctx
                .read(|r| ArtifactRepository::new(r)?.load_template(source.id))
                .map_err(|_| Code::RuntimeRejected)?
                .map_err(|_| Code::RepositoryRejected)?;
            if loaded.source() != &source.token {
                return Err(Code::WrongBinding.into());
            }
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
            proof: None,
            release_first: None,
            release_latest: None,
            handoff_receipt: None,
            handoff_failure: None,
            discarded_input: None,
        };
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
    match &*job.input {
        Work::TemplateDraftContent {
            snapshot, offset, ..
        } => {
            let content = chunk(entry.snapshot, &entry.content, *snapshot, offset)?;
            Ok(Completed::new(
                (),
                Ok(ResultDto::TemplateDraftContent { content }),
            ))
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
            if matches!(entry.phase, "uncertain" | "saved_read_required") {
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
            let captured = recovery::capture_assets(ctx, job, &frozen, &mut notes);
            entry.original = Some(Box::new((entry.original.take(), notes)));
            if let Err(error) = captured {
                entry.error = Some(error);
                return reply(entry);
            }
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
            entry.submitted = Some(record);
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
                let store = job.recovery.connect()?;
                let proof = store
                    .lock()
                    .map_err(|_| {
                        Arc::new(crate::data::edit_recovery::error::RecoveryError::new(
                            crate::data::edit_recovery::error::Category::Unavailable,
                            crate::data::edit_recovery::error::Stage::Write,
                        ))
                    })?
                    .accept(&deposit)
                    .map_err(Arc::new)?;
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
            if let Some(body) = body {
                entry.accept_body(generation, body)?;
            }
            if parse_generation(generation)? != entry.generation {
                return Err(Code::WrongBinding.into());
            }
            if !discard
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
