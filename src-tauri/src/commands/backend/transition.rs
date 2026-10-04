//! Transition planning keeps legacy inputs separate from canonical authority.
use super::*;
use crate::data::edit_recovery::model::{Deposit, Draft, Envelope, OriginalKind};

/// One old compound save becomes a Template input and a Document input. The
/// original operation evidence remains unchanged in both components and backup.
/// New field identities use the existing deterministic allocator in both paths.
pub(super) fn components(envelope: &Envelope) -> Reply<Vec<Envelope>> {
    let mut document = envelope.clone();
    let (document_id, template_id) = match &envelope.draft {
        Draft::AdmittedComposite {
            document, template, ..
        }
        | Draft::AdmittedDocument {
            document, template, ..
        } => (Some(document.clone()), template.clone()),
        Draft::Document {
            document, template, ..
        } => (document.clone(), template.clone()),
        Draft::Template { .. } => return Ok(vec![envelope.clone()]),
    };
    let mut body = recovery_document::body(&envelope.draft)?;
    let mut result = Vec::new();
    if matches!(envelope.draft, Draft::AdmittedComposite { .. }) {
        let original = envelope
            .originals
            .iter()
            .find(|original| original.kind == OriginalKind::Template)
            .ok_or(Code::WrongBinding)?;
        let template = artifact::decode_template(original.snapshot.as_bytes())
            .map_err(|_| Code::RecoveryRejected)?;
        workspace::recovery_template::map_composite_document(envelope, &template, &mut body)?;
        let preview = workspace::recovery_template::composite_preview(envelope)?;
        let mut component = envelope.clone();
        component
            .originals
            .retain(|original| original.kind == OriginalKind::Template);
        component.draft = preview.recovery(Some(template_id.clone()));
        Deposit::freeze(component.clone()).map_err(|_| Code::RecoveryRejected)?;
        result.push(component);
    }
    document.draft = Draft::Document {
        document: document_id,
        template: template_id,
        name: body.name,
        english_name: body.english_name,
        glossary_summary: body.glossary_summary,
        glossary_excluded: body.glossary_excluded,
        fields: body.fields,
        composing: body.composing,
    };
    Deposit::freeze(document.clone()).map_err(|_| Code::RecoveryRejected)?;
    result.push(document);
    Ok(result)
}

pub(super) fn staged(
    input: &crate::data::edit_recovery::transition::StagedInput,
    backup: &crate::data::edit_recovery::transition::VerifiedBackup,
) -> Reply<Envelope> {
    input
        .read_verified(backup)
        .map_err(|_| Code::RecoveryRejected.into())
}

use crate::data::edit_recovery::transition::{Prepared, StagedInput};
use crate::data::repository::{drafts, versions};
use std::collections::BTreeMap;

/// Admission cannot rely on a prior list request. Direct reads/begins use the
/// same verified backup and exact multi-target transaction before issuing views.
pub(super) fn policy(ctx: &mut Context, job: &Job) -> Reply<Option<Completed>> {
    let pending = ctx
        .read(|ready| ArtifactRepository::new(ready)?.policy_transition_sources())
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    if pending.is_empty() {
        return Ok(None);
    }
    if ctx.has_project_owners() {
        return Err(Code::SessionRejected.into());
    }
    let connection = job.recovery.connect().map_err(|_| Code::SinkUnavailable)?;
    let store = connection.lock().map_err(|_| Code::Unavailable)?;
    let prepared = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
            store.prepare_policy_transition(&repository)
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RecoveryRejected)?;
    let Some(prepared) = prepared else {
        return Ok(None);
    };
    let targets = prepared
        .sources
        .iter()
        .map(|(id, _)| id.path())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Code::WrongBinding)?;
    let (binding, registration) = begin(ctx, job, targets)?;
    let Some(key) = &binding.key else {
        let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
        let mut failure = Completed::reject((registration, cleanup), Code::SessionRejected.into());
        if !released {
            failure.binding = Some(binding);
        }
        return Ok(Some(failure));
    };
    let outcome = match ctx.session(key) {
        Ok(mut session) => session.change_policy_formats(&prepared),
        Err(error) => {
            let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
            let mut failure =
                Completed::reject((registration, error, cleanup), Code::SessionRejected.into());
            if !released {
                failure.binding = Some(binding);
            }
            return Ok(Some(failure));
        }
    };
    let committed = outcome
        .as_ref()
        .is_ok_and(|write| write.diagnostic().disk == DiskState::Committed);
    let preserved = prepared.validate().is_ok();
    let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
    if !committed || !released || !preserved {
        let mut failure = Completed::reject(
            (registration, outcome, cleanup),
            Code::RecoveryRejected.into(),
        );
        if !released {
            failure.binding = Some(binding);
        }
        return Ok(Some(failure));
    }
    // Keep the independently verified original handles alive through commit and
    // owner release. A retry after interruption reuses the same immutable backup.
    drop(prepared);
    let remaining = ctx
        .read(|ready| ArtifactRepository::new(ready)?.policy_transition_sources())
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    if !remaining.is_empty() {
        return Err(Code::RecoveryRejected.into());
    }
    Ok(None)
}

fn issued(draft: &str, kind: &str) -> Reply<String> {
    workspace::allocated_id(
        draft,
        &format!("transition-{kind}"),
        &format!("new:{draft}"),
    )
}
fn source_id(envelope: &Envelope) -> Reply<ArtifactSourceId> {
    match &envelope.draft {
        Draft::Template {
            template: Some(id), ..
        } => Ok(ArtifactSourceId::Template(convert::id(id)?)),
        Draft::Document {
            document: Some(id), ..
        } => Ok(ArtifactSourceId::Document(convert::id(id)?)),
        _ => Err(Code::RecoveryRejected.into()),
    }
}
fn read_template(
    ctx: &mut Context,
    id: artifact::TemplateId,
) -> Reply<crate::data::repository::LoadedArtifact<artifact::TemplateArtifact>> {
    ctx.read(|ready| ArtifactRepository::new(ready)?.load_template(id))
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected.into())
}

fn create_template(
    ctx: &mut Context,
    job: &Job,
    input: &StagedInput,
    envelope: &mut Envelope,
) -> Reply<Option<Completed>> {
    let Draft::Template { template, name, .. } = &envelope.draft else {
        return Err(Code::WrongBinding.into());
    };
    if template.is_some() {
        return Ok(None);
    }
    let id_text = issued(&envelope.key.draft_id, "template")?;
    let id = convert::id(&id_text)?;
    let time = envelope.created_at_utc.clone();
    let seed = artifact::template_mutation::whole::unpublished_seed(id, time.clone())
        .map_err(|_| Code::PreparationRejected)?;
    let draft = artifact::template_mutation::whole::TemplateDraftInput {
        sections: vec![],
        name: if name.trim().is_empty() {
            "새 템플릿".into()
        } else {
            name.clone()
        },
        glossary_excluded: false,
        presentation_token: None,
        fields: vec![],
    };
    let candidate =
        artifact::template_mutation::whole::prepare_new_template_draft(&seed, &time, draft.clone())
            .map_err(|_| Code::PreparationRejected)?;
    let bytes = artifact::encode_template(&candidate).map_err(|_| Code::SerializationFailed)?;
    let existing = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready)?;
            repository.scan_templates().map(|scan| {
                scan.records()
                    .iter()
                    .any(|record| record.source().id() == ArtifactSourceId::Template(id))
            })
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    input
        .creation_intent("template", &id_text, &bytes, !existing)
        .map_err(|_| Code::RecoveryRejected)?;
    if !existing {
        let mut prepared = ctx
            .prepare_whole_template(&seed, &time, draft)
            .map_err(|_| Code::PreparationRejected)?;
        let (binding, registration) = begin(ctx, job, prepared.session_targets().to_vec())?;
        let Some(key) = &binding.key else {
            let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
            let mut failure = Completed::reject(
                (prepared, registration, cleanup),
                Code::SessionRejected.into(),
            );
            if !released {
                failure.binding = Some(binding);
            }
            return Ok(Some(failure));
        };
        let outcome = match ctx.session(key) {
            Ok(mut session) => session.create_template(&mut prepared),
            Err(error) => {
                let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
                let mut failure = Completed::reject(
                    (prepared, registration, error, cleanup),
                    Code::SessionRejected.into(),
                );
                if !released {
                    failure.binding = Some(binding);
                }
                return Ok(Some(failure));
            }
        };
        let committed = outcome.diagnostic().disk == DiskState::Committed;
        let (released, cleanup) = document_workspace::finish_session(ctx, &binding);
        if !committed || !released {
            let mut failure = Completed::reject(
                (prepared, registration, outcome, cleanup),
                Code::RecoveryRejected.into(),
            );
            if !released {
                failure.binding = Some(binding);
            }
            return Ok(Some(failure));
        }
    }
    let loaded = read_template(ctx, id)?;
    if artifact::encode_template(loaded.artifact()).map_err(|_| Code::SerializationFailed)? != bytes
    {
        return Err(Code::WrongBinding.into());
    }
    let original =
        recovery::original(&View::Template(loaded)).map_err(|_| Code::RecoveryRejected)?;
    envelope.originals = vec![original];
    if let Draft::Template { template, .. } = &mut envelope.draft {
        *template = Some(id_text);
    }
    Ok(None)
}

fn create_document(
    ctx: &mut Context,
    job: &Job,
    input: &StagedInput,
    envelope: &mut Envelope,
) -> Reply<Option<Completed>> {
    let Draft::Document {
        document,
        template,
        name,
        ..
    } = &envelope.draft
    else {
        return Err(Code::WrongBinding.into());
    };
    if document.is_some() {
        return Ok(None);
    }
    let prior_candidate = envelope
        .originals
        .iter()
        .find(|value| value.kind == OriginalKind::Document);
    if prior_candidate.is_some_and(|candidate| {
        !envelope.attempt.as_ref().is_some_and(|attempt| {
            attempt.candidate_digest.as_ref() == Some(&candidate.snapshot_digest)
        })
    }) {
        return Err(Code::RecoveryRejected.into());
    }
    let id_text = prior_candidate
        .map(|candidate| candidate.artifact_id.clone())
        .map(Ok)
        .unwrap_or_else(|| issued(&envelope.key.draft_id, "document"))?;
    let id = convert::id(&id_text)?;
    let template = read_template(ctx, convert::id(template)?)?;
    let template_source = templates::TemplateSource {
        id: template.artifact().template_id(),
        token: template.source().clone(),
        expected_revision: template.artifact().revision(),
    };
    let time = envelope.created_at_utc.clone();
    let name = match name {
        crate::data::edit_recovery::model::Intent::Set(name) if !name.trim().is_empty() => {
            name.clone()
        }
        _ => "새 문서".into(),
    };
    let candidate = artifact::create_document_with_term_info(
        template.artifact(),
        template.artifact().revision(),
        id,
        name,
        String::new(),
        String::new(),
        false,
        time.clone(),
        &BTreeMap::new(),
    )
    .map_err(|_| Code::PreparationRejected)?
    .into_document();
    let bytes = artifact::encode_document(&candidate).map_err(|_| Code::SerializationFailed)?;
    let (documents, base) = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready)?;
            let documents = repository.scan_documents()?;
            let layout = repository.load_layout()?;
            let base = layout
                .map(|layout| crate::data::application::layout::LayoutSnapshot {
                    layout: layout.artifact().clone(),
                    source: Some(layout.source().clone()),
                })
                .unwrap_or_else(|| crate::data::application::layout::LayoutSnapshot {
                    layout: artifact::layout::DocumentLayout::flat(
                        documents.summaries().map(|(id, _, _, _, _, _)| id),
                    ),
                    source: None,
                });
            Ok::<_, crate::data::repository::RepositoryError>((documents, base))
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    let existing = documents.source(id).is_some();
    if existing && prior_candidate.is_some() {
        let candidate = prior_candidate.ok_or(Code::WrongBinding)?;
        let loaded = ctx
            .read(|ready| ArtifactRepository::new(ready)?.load_document(id))
            .map_err(|_| Code::RuntimeRejected)?
            .map_err(|_| Code::RepositoryRejected)?;
        if artifact::encode_document(loaded.artifact()).map_err(|_| Code::SerializationFailed)?
            != candidate.snapshot.as_bytes()
        {
            return Err(Code::WrongBinding.into());
        }
        envelope.originals = vec![
            recovery::original(&View::Template(template)).map_err(|_| Code::RecoveryRejected)?,
            recovery::original(&View::Document(loaded)).map_err(|_| Code::RecoveryRejected)?,
        ];
        if let Draft::Document { document, .. } = &mut envelope.draft {
            *document = Some(id_text);
        }
        return Ok(None);
    }
    input
        .creation_intent("document", &id_text, &bytes, !existing)
        .map_err(|_| Code::RecoveryRejected)?;
    if !existing {
        let write = crate::data::application::layout::LayoutWrite {
            base,
            documents: Arc::new(documents),
            edit: artifact::layout::LayoutEdit::Adopt {},
            timestamp: time,
            create: Some((candidate, template_source, None)),
        };
        let (completed, retained, _, committed) =
            document_workspace::write_layout(ctx, job, &write, None)?;
        if retained.is_some()
            || committed.is_none()
            || !matches!(
                completed.dto,
                Ok(ResultDto::Write {
                    disk: DiskDto::Committed,
                    ..
                })
            )
        {
            let mut failure = Completed::reject(completed, Code::RecoveryRejected.into());
            failure.binding = retained;
            return Ok(Some(failure));
        }
    }
    let loaded = ctx
        .read(|ready| ArtifactRepository::new(ready)?.load_document(id))
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RepositoryRejected)?;
    if artifact::encode_document(loaded.artifact()).map_err(|_| Code::SerializationFailed)? != bytes
    {
        return Err(Code::WrongBinding.into());
    }
    envelope.originals = vec![
        recovery::original(&View::Template(template)).map_err(|_| Code::RecoveryRejected)?,
        recovery::original(&View::Document(loaded)).map_err(|_| Code::RecoveryRejected)?,
    ];
    if let Draft::Document { document, .. } = &mut envelope.draft {
        *document = Some(id_text);
    }
    Ok(None)
}

/// The staged/whole-backup phase precedes target creation and every legacy delete.
/// Raw input is attached, never automatically saved into an existing canonical item.
pub(super) fn run(ctx: &mut Context, job: &Job) -> Reply<Option<Completed>> {
    if ctx.has_project_owners() {
        return Ok(None);
    }
    let required = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
            job.recovery.transition_required(&repository)
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RecoveryRejected)?;
    if !required {
        return Ok(None);
    }
    let connection = job.recovery.connect().map_err(|_| Code::SinkUnavailable)?;
    let mut store = connection.lock().map_err(|_| Code::Unavailable)?;
    ctx.read(|ready| {
        let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
        store.transition_format_history(&repository)
    })
    .map_err(|_| Code::RuntimeRejected)?
    .map_err(|_| Code::RecoveryRejected)?;
    let prepared = ctx
        .read(|ready| {
            let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
            store.prepare_if_present(&repository)
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RecoveryRejected)?;
    let Some(prepared) = prepared else {
        return Ok(None);
    };
    if prepared.completed() {
        prepared
            .finish(&mut store)
            .map_err(|_| Code::RecoveryRejected)?;
        return Ok(None);
    }
    apply(ctx, job, &mut store, &prepared)
}
fn bind_created(
    ctx: &mut Context,
    job: &Job,
    input: &StagedInput,
    envelope: &mut Envelope,
) -> Reply<Option<Completed>> {
    match &envelope.draft {
        Draft::Template { template: None, .. } => create_template(ctx, job, input, envelope),
        Draft::Document { document: None, .. } => create_document(ctx, job, input, envelope),
        _ => Ok(None),
    }
}
fn import_originals(ctx: &mut Context, input: &StagedInput, envelope: &Envelope) -> Reply<()> {
    ctx.read(|ready| {
        let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
        for original in &envelope.originals {
            if original.kind == OriginalKind::Document
                && matches!(envelope.draft, Draft::Document { document: None, .. })
            {
                continue;
            }
            let target = match original.kind {
                OriginalKind::Template => ArtifactSourceId::Template(
                    original
                        .artifact_id
                        .parse()
                        .map_err(std::io::Error::other)?,
                ),
                OriginalKind::Document => ArtifactSourceId::Document(
                    original
                        .artifact_id
                        .parse()
                        .map_err(std::io::Error::other)?,
                ),
            };
            let interpretation = if original.kind == OriginalKind::Document {
                envelope
                    .originals
                    .iter()
                    .find(|value| value.kind == OriginalKind::Template)
                    .map(|value| value.snapshot.clone())
            } else {
                None
            };
            input.import_original(
                &repository,
                target,
                original.snapshot.as_bytes(),
                interpretation,
                &envelope.created_at_utc,
            )?;
        }
        Ok::<_, std::io::Error>(())
    })
    .map_err(|_| Code::RuntimeRejected)?
    .map_err(|_| Code::RecoveryRejected)?;
    Ok(())
}
fn apply(
    ctx: &mut Context,
    job: &Job,
    store: &mut crate::data::edit_recovery::Store,
    prepared: &Prepared,
) -> Reply<Option<Completed>> {
    let verified = prepared.verify().map_err(|_| Code::RecoveryRejected)?;
    // Keep only identities/order metadata resident. Large raw envelopes are
    // verified and streamed from the independent snapshot one at a time.
    let mut by_draft = BTreeMap::<String, (usize, u64)>::new();
    let mut ordered = Vec::new();
    for (index, input) in prepared.inputs.iter().enumerate() {
        let envelope = staged(input, &verified)?;
        if by_draft
            .get(&envelope.key.draft_id)
            .is_none_or(|(_, generation)| envelope.key.generation > *generation)
        {
            by_draft.insert(
                envelope.key.draft_id.clone(),
                (index, envelope.key.generation),
            );
        }
        ordered.push((
            envelope.created_at_utc.clone(),
            envelope.key.draft_id.clone(),
            envelope.key.generation,
            index,
        ));
    }
    ordered.sort();
    // A creation candidate in an unresolved attempt is not a canonical source.
    // Require the actual recovered file to match before promoting it to a version
    // or retiring its old input. Missing/mismatched files preserve the whole input.
    for (_, _, _, index) in &ordered {
        let envelope = staged(&prepared.inputs[*index], &verified)?;
        if let Some(attempt) = envelope
            .attempt
            .as_ref()
            .filter(|attempt| attempt.unresolved())
        {
            if let Some(candidate) = envelope.originals.iter().find(|original| {
                original.kind == OriginalKind::Document
                    && attempt.candidate_digest.as_ref() == Some(&original.snapshot_digest)
            }) {
                let same = ctx
                    .read(|ready| {
                        let repository =
                            ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
                        let loaded = repository
                            .load_document(
                                candidate
                                    .artifact_id
                                    .parse()
                                    .map_err(std::io::Error::other)?,
                            )
                            .map_err(std::io::Error::other)?;
                        let actual = artifact::encode_document(loaded.artifact())
                            .map_err(std::io::Error::other)?;
                        Ok::<_, std::io::Error>(
                            actual == candidate.snapshot.as_bytes()
                                && repository
                                    .reread_bytes_match(loaded.source())
                                    .map_err(std::io::Error::other)?,
                        )
                    })
                    .map_err(|_| Code::RuntimeRejected)?
                    .map_err(|_| Code::RecoveryRejected)?;
                if !same {
                    return Err(Code::RecoveryRejected.into());
                }
            }
        }
    }
    let mut latest = BTreeMap::<ArtifactSourceId, (usize, usize, String, String, u64)>::new();
    for (index, _) in by_draft.values() {
        let input = &prepared.inputs[*index];
        let envelope = staged(input, &verified)?;
        for (component, mut part) in components(&envelope)?.into_iter().enumerate() {
            if let Some(failure) = bind_created(ctx, job, input, &mut part)? {
                return Ok(Some(failure));
            }
            let id = source_id(&part)?;
            let keep = latest
                .get(&id)
                .is_none_or(|(_, _, time, draft, generation)| {
                    if *draft == part.key.draft_id {
                        part.key.generation > *generation
                    } else {
                        (&part.created_at_utc, &part.key.draft_id) > (time, draft)
                    }
                });
            if keep {
                latest.insert(
                    id,
                    (
                        *index,
                        component,
                        part.created_at_utc,
                        part.key.draft_id,
                        part.key.generation,
                    ),
                );
            }
        }
    }
    for (_, _, _, index) in &ordered {
        let input = &prepared.inputs[*index];
        let envelope = staged(input, &verified)?;
        import_originals(ctx, input, &envelope)?;
    }
    for (id, (index, component, _, _, _)) in &latest {
        let input = &prepared.inputs[*index];
        let envelope = staged(input, &verified)?;
        let mut part = components(&envelope)?
            .into_iter()
            .nth(*component)
            .ok_or(Code::WrongBinding)?;
        if let Some(failure) = bind_created(ctx, job, input, &mut part)? {
            return Ok(Some(failure));
        }
        if source_id(&part)? != *id {
            return Err(Code::WrongBinding.into());
        }
        import_originals(ctx, input, &part)?;
        ctx.read(|ready| {
            let repository = ArtifactRepository::new(ready).map_err(std::io::Error::other)?;
            let existing = drafts::latest(&repository, *id)?;
            let replace = existing.as_ref().is_none_or(|current| {
                if current.key.draft_id == part.key.draft_id {
                    part.key.generation > current.key.generation
                } else {
                    (&part.created_at_utc, &part.key.draft_id)
                        > (&current.created_at_utc, &current.key.draft_id)
                }
            });
            if replace {
                input.restore_live_assets(
                    &verified,
                    ready.locked_project().canonical_root(),
                    &part.draft,
                )?;
                if existing.is_none()
                    && match id {
                        ArtifactSourceId::Template(id) => repository.load_template(*id).is_ok(),
                        ArtifactSourceId::Document(id) => repository.load_document(*id).is_ok(),
                        _ => false,
                    }
                {
                    versions::confirm(&repository, *id, &part.created_at_utc)?;
                }
                drafts::preserve(&repository, part.clone())?;
            }
            let durable = drafts::latest(&repository, *id)?
                .ok_or_else(|| std::io::Error::other("transition input not durable"))?;
            let expected = if replace {
                &part
            } else {
                existing
                    .as_ref()
                    .ok_or_else(|| std::io::Error::other("transition latest receipt missing"))?
            };
            if durable != *expected {
                return Err(std::io::Error::other(
                    "transition latest input differs from accepted receipt",
                ));
            }
            Ok::<_, std::io::Error>(())
        })
        .map_err(|_| Code::RuntimeRejected)?
        .map_err(|_| Code::RecoveryRejected)?;
    }
    // Verify every exact independent copy again. No earlier source is discarded
    // if a later component, its backup, or the target-local publication failed.
    drop(verified);
    let verified = prepared.verify().map_err(|_| Code::RecoveryRejected)?;
    for input in &prepared.inputs {
        staged(input, &verified)?;
    }
    drop(verified);
    prepared.finish(store).map_err(|_| Code::RecoveryRejected)?;
    Ok(None)
}
