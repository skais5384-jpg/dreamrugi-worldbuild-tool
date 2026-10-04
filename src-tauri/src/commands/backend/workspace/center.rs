use super::*;
use crate::commands::workspace::{ReapplyIntent, RecoverySelection};
use crate::data::edit_recovery::{
    error::RecoveryError,
    model::{Deposit, Draft},
    Owner,
};
use std::sync::Mutex;

pub(super) struct Selected {
    deposit: Deposit,
    content: String,
    phase: &'static str,
    current: Option<Arc<View>>,
    intents: Vec<ReapplyIntent>,
    comparison: Option<super::super::recovery_merge::Plan>,
    basis: Option<super::super::recovery_merge::Apply>,
    template_part: Option<TemplateBody>,
}
impl Selected {
    fn dto(&self, snapshot: Id) -> RecoverySelection {
        RecoverySelection {
            snapshot,
            key: self.deposit.key().clone(),
            phase: self.phase,
            can_restore: true,
            intents: self.intents.clone(),
        }
    }
    fn rebuild(&mut self) -> Reply<()> {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Content<'a> {
            draft: &'a Draft,
            original: Option<TemplateDto>,
            current: Option<TemplateDto>,
            attempt: &'a Option<crate::data::edit_recovery::model::Attempt>,
            comparison: Option<&'a [super::super::recovery_merge::Change]>,
            basis: &'a Option<super::super::recovery_merge::Apply>,
            template_part: bool,
        }
        let original = self
            .deposit
            .envelope()
            .originals
            .iter()
            .find(|o| {
                matches!(
                    o.kind,
                    crate::data::edit_recovery::model::OriginalKind::Template
                )
            })
            .map(|o| {
                artifact::decode_template(o.snapshot.as_bytes())
                    .map_err(|_| Code::RecoveryRejected)
                    .and_then(|t| projection::template(&t).map_err(|e| e.code))
            })
            .transpose()?;
        let current = self
            .current
            .as_ref()
            .map(|v| match &**v {
                View::Template(t) => projection::template(t.artifact()),
                _ => Err(Code::WrongBinding.into()),
            })
            .transpose()?;
        self.content = serde_json::to_string(&Content {
            draft: &self.deposit.envelope().draft,
            original,
            current,
            attempt: &self.deposit.envelope().attempt,
            comparison: self.comparison.as_ref().map(|plan| plan.changes.as_slice()),
            basis: &self.basis,
            template_part: self.template_part.is_some(),
        })
        .map_err(|_| Code::SerializationFailed)?;
        if self.content.len() > crate::data::edit_recovery::model::MAX_FILE_BYTES {
            return Err(Code::Full.into());
        }
        Ok(())
    }
}
pub(crate) fn is_local(work: &Work) -> bool {
    matches!(
        work,
        Work::RecoveryPage { .. }
            | Work::RecoveryCloseCursor { .. }
            | Work::RecoveryRead { .. }
            | Work::RecoveryContent { .. }
            | Work::RecoveryReleaseSelection { .. }
            | Work::RecoveryRevalidate { .. }
            | Work::RecoveryDiscard { .. }
    )
}
fn failure(error: Arc<RecoveryError>) -> Completed {
    let dto = error.dto();
    Completed::new(error, Ok(ResultDto::RecoveryFailure { failure: dto }))
}
pub(crate) fn run(
    input: Arc<Work>,
    owner: Arc<Owner>,
    registry: Arc<Mutex<Registry>>,
) -> Completed {
    let result = (|| -> Reply<Completed> {
        let mut registry = registry.lock().map_err(|_| Code::Unavailable)?;
        match &*input {
            Work::RecoveryContent { snapshot, offset } => {
                let selected = registry.selections.get(snapshot).ok_or(Code::UnknownId)?;
                return Ok(Completed::new(
                    (),
                    Ok(ResultDto::TemplateDraftContent {
                        content: operations::chunk(
                            *snapshot,
                            &selected.content,
                            *snapshot,
                            offset,
                        )?,
                    }),
                ));
            }
            Work::RecoveryReleaseSelection { snapshot } => {
                let original = registry.selections.remove(snapshot);
                return Ok(Completed::new(
                    original,
                    Ok(ResultDto::Control { error: None }),
                ));
            }
            _ => (),
        }
        let legacy = if matches!(&*input, Work::RecoveryPage { cursor: None }) {
            match owner.handoff_legacy() {
                Ok(summary) => summary,
                Err(error) => return Ok(failure(error)),
            }
        } else {
            None
        };
        let store = match owner.connect() {
            Ok(s) => s,
            Err(e) => return Ok(failure(e)),
        };
        let mut store = store.lock().map_err(|_| Code::Unavailable)?;
        match &*input {
            Work::RecoveryPage { cursor } => Ok(match store.page(cursor.as_deref()) {
                Ok(mut page) => {
                    page.legacy = legacy;
                    Completed::new((), Ok(ResultDto::RecoveryPage { page }))
                }
                Err(e) => failure(Arc::new(e)),
            }),
            Work::RecoveryCloseCursor { cursor } => {
                store.close_cursor(cursor);
                Ok(Completed::new((), Ok(ResultDto::Control { error: None })))
            }
            Work::RecoveryRead {
                key,
                deposit_id,
                digest: expected,
            } => {
                if registry.selections.len() >= 8 {
                    return Err(Code::Full.into());
                }
                let deposit = match store.read(key, deposit_id) {
                    Ok(d) => d,
                    Err(e) => return Ok(failure(Arc::new(e))),
                };
                if deposit.payload_digest() != expected {
                    return Err(Code::WrongBinding.into());
                }
                let snapshot = Id::new();
                let mut selected = Selected {
                    deposit,
                    content: String::new(),
                    phase: "unchecked",
                    current: None,
                    intents: vec![],
                    comparison: None,
                    basis: None,
                    template_part: None,
                };
                selected.rebuild()?;
                let dto = selected.dto(snapshot);
                registry.selections.insert(snapshot, selected);
                Ok(Completed::new(
                    (),
                    Ok(ResultDto::RecoverySelection { selection: dto }),
                ))
            }
            Work::RecoveryRevalidate {
                key,
                deposit_id,
                digest,
            } => Ok(match store.revalidate(key, deposit_id, digest) {
                Ok(proof) => Completed::new(
                    proof,
                    Ok(ResultDto::RecoveryReceipt {
                        receipt: ReceiptDto {
                            key: key.clone(),
                            deposit_id: deposit_id.clone(),
                            digest: digest.clone(),
                        },
                    }),
                ),
                Err(e) => failure(Arc::new(e)),
            }),
            Work::RecoveryDiscard { key, version } => {
                if registry.documents.retains(key)
                    || registry.entries.values().any(|e| {
                        e.restored_from.as_ref() == Some(key)
                            || e.submitted.as_ref().is_some_and(|r| &r.envelope.key == key)
                            || e.deposit.as_ref().is_some_and(|r| &r.envelope.key == key)
                    })
                {
                    return Err(Code::OwnersRemain.into());
                }
                Ok(match store.discard(key, version) {
                    Ok(()) => Completed::new((), Ok(ResultDto::Control { error: None })),
                    Err(e) => failure(Arc::new(e)),
                })
            }
            _ => Err(Code::InvalidInput.into()),
        }
    })();
    result.unwrap_or_else(|e| Completed::reject(input, e))
}

pub(super) fn restore(ctx: &mut Context, job: &Job) -> Reply<Completed> {
    let Work::RecoveryRestore {
        project,
        snapshot,
        reapply,
    } = &*job.input
    else {
        return Err(Code::InvalidInput.into());
    };
    let mut registry = job.workspace.lock().map_err(|_| Code::Unavailable)?;
    if registry.entries.len() >= 16 {
        return Err(Code::Full.into());
    }
    let selected = registry.selections.get(snapshot).ok_or(Code::UnknownId)?;
    let key = selected.deposit.key();
    if ctx.project_fingerprint() != Some(key.project_fingerprint.as_str()) {
        return Err(Code::WrongBinding.into());
    }
    // 이 lock은 중복 검사부터 최종 등록까지 유지된다. release 실패 owner도 남아
    // 있으므로 다른 snapshot/세대/operation으로 같은 초안을 우회 등록할 수 없다.
    if registry
        .entries
        .values()
        .any(|entry| entry.fingerprint == key.project_fingerprint && entry.draft_id == key.draft_id)
    {
        return Err(Code::DuplicateConflict.into());
    }
    let selected = registry
        .selections
        .get_mut(snapshot)
        .ok_or(Code::UnknownId)?;
    let envelope = selected.deposit.envelope().clone();
    let store = match job.recovery.connect() {
        Ok(s) => s,
        Err(e) => return Ok(failure(e)),
    };
    let locked_store = store.lock().map_err(|_| Code::Unavailable)?;
    let current_record = locked_store.read(&envelope.key, &envelope.deposit_id);
    match current_record {
        Ok(record) if record.payload_digest() == selected.deposit.payload_digest() => (),
        Ok(record) => return Ok(Completed::reject(record, Code::WrongBinding.into())),
        Err(e) => return Ok(failure(Arc::new(e))),
    }
    let latest = match locked_store.latest_generation(&envelope.key) {
        Ok(generation) => generation,
        Err(e) => return Ok(failure(Arc::new(e))),
    };
    let generation = latest.checked_add(1).ok_or(Code::Full)?;
    // 기존 project worker 직렬화와 workspace lock이 scan 뒤의 보관/등록도 보호한다.
    // store lock은 begin_owner의 복구 연결 전에 놓아 잠금 순서를 역전하지 않는다.
    drop(locked_store);
    if envelope.attempt.as_ref().is_some_and(|a| {
        matches!(
            a.result,
            crate::data::edit_recovery::model::SaveState::Unknown
                | crate::data::edit_recovery::model::SaveState::Uncertain
        )
    }) {
        selected.phase = "uncertain";
        selected.rebuild()?;
        return Ok(Completed::new(
            (),
            Ok(ResultDto::RecoverySelection {
                selection: selected.dto(*snapshot),
            }),
        ));
    }
    let component_request = reapply
        .as_ref()
        .is_some_and(|choices| choices.as_slice() == [ReapplyIntent::ComponentTemplate]);
    if component_request {
        selected.template_part = Some(super::recovery_template::composite_preview(&envelope)?);
        selected.basis = None;
    }
    if !component_request && reapply.is_none() && selected.template_part.is_some() {
        selected.template_part = None;
        selected.basis = None;
        selected.current = None;
        selected.comparison = None;
    }
    if !matches!(envelope.draft, Draft::Template { .. }) && selected.template_part.is_none() {
        let (template_id, document_id) = match &envelope.draft {
            Draft::Document {
                template, document, ..
            } => (template, document.as_deref()),
            Draft::AdmittedDocument {
                template, document, ..
            }
            | Draft::AdmittedComposite {
                template, document, ..
            } => (template, Some(document.as_str())),
            _ => return Err(Code::InvalidInput.into()),
        };
        let template_id = convert::id(template_id)?;
        let loaded =
            match ctx.read(|ready| ArtifactRepository::new(ready)?.load_template(template_id)) {
                Ok(Ok(template)) => template,
                original => {
                    selected.phase = "missing_or_unreadable";
                    selected.rebuild()?;
                    return Ok(Completed::new(
                        original,
                        Ok(ResultDto::RecoverySelection {
                            selection: selected.dto(*snapshot),
                        }),
                    ));
                }
            };
        if loaded.artifact().lifecycle() != artifact::TemplateLifecycle::Active {
            selected.phase = "deleted";
            selected.rebuild()?;
            return Ok(Completed::new(
                loaded,
                Ok(ResultDto::RecoverySelection {
                    selection: selected.dto(*snapshot),
                }),
            ));
        }
        let document = if let Some(id) = document_id {
            let id = convert::id(id)?;
            match ctx.read(|ready| ArtifactRepository::new(ready)?.load_document(id)) {
                Ok(Ok(document)) => Some(document),
                original => {
                    selected.phase = "missing_or_unreadable";
                    selected.rebuild()?;
                    return Ok(Completed::new(
                        original,
                        Ok(ResultDto::RecoverySelection {
                            selection: selected.dto(*snapshot),
                        }),
                    ));
                }
            }
        } else {
            None
        };
        let plan = super::super::recovery_document::plan(
            &envelope,
            loaded.artifact(),
            document.as_ref().map(|d| d.artifact()),
        )?;
        let digest = |token: &crate::data::repository::SourceToken| {
            token
                .sha256()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        selected.basis = Some(super::super::recovery_merge::Apply {
            template_digest: digest(loaded.source()),
            document_digest: document.as_ref().map(|d| digest(d.source())),
            selected: vec![],
        });
        selected.intents = plan
            .changes
            .iter()
            .filter(|change| change.status != "blocked")
            .map(|change| ReapplyIntent::Change {
                change: change.id.clone(),
            })
            .collect();
        selected.comparison = Some(plan);
        selected.current = Some(Arc::new(View::Template(loaded)));
        selected.phase = "conflict";
        selected.rebuild()?;
        return Ok(Completed::new(
            document,
            Ok(ResultDto::RecoverySelection {
                selection: selected.dto(*snapshot),
            }),
        ));
    }
    let Some(mut body) = selected
        .template_part
        .clone()
        .or_else(|| TemplateBody::from_recovery(&envelope.draft))
    else {
        return Err(Code::InvalidInput.into());
    };
    let template = match &envelope.draft {
        Draft::Template { template, .. } => template.clone(),
        Draft::AdmittedComposite { template, .. } => Some(template.clone()),
        _ => return Err(Code::InvalidInput.into()),
    };
    let base = if let Some(id) = &template {
        let id = convert::id(id)?;
        let loaded = match ctx.read(|r| ArtifactRepository::new(r)?.load_template(id)) {
            Ok(Ok(loaded)) => loaded,
            original => {
                selected.phase = "missing_or_unreadable";
                return Ok(Completed::new(
                    original,
                    Ok(ResultDto::RecoverySelection {
                        selection: selected.dto(*snapshot),
                    }),
                ));
            }
        };
        let original = envelope
            .originals
            .iter()
            .find(|o| o.kind == crate::data::edit_recovery::model::OriginalKind::Template)
            .ok_or(Code::WrongBinding)?;
        let token = loaded.source();
        let actual_digest: String = token.sha256().iter().map(|b| format!("{b:02x}")).collect();
        let same = actual_digest == original.source_digest
            && token.byte_length() as u64 == original.source_byte_length
            && token.schema().get() == original.schema
            && loaded.artifact().revision().get() == original.template_revision;
        if loaded.artifact().lifecycle() != artifact::TemplateLifecycle::Active {
            selected.phase = "deleted";
            return Ok(Completed::new(
                loaded,
                Ok(ResultDto::RecoverySelection {
                    selection: selected.dto(*snapshot),
                }),
            ));
        }
        if !same || selected.template_part.is_some() {
            let reapply_current = selected
                .current
                .as_ref()
                .is_some_and(|v| v.template().is_ok_and(|s| s.token == *token));
            if reapply.is_none() || component_request || !reapply_current {
                let base = artifact::decode_template(original.snapshot.as_bytes())
                    .map_err(|_| Code::RecoveryRejected)?;
                let plan = super::recovery_template::plan(&body, &base, loaded.artifact())?;
                selected.intents = plan
                    .changes
                    .iter()
                    .map(|change| ReapplyIntent::Change {
                        change: change.id.clone(),
                    })
                    .collect();
                selected.comparison = Some(plan);
                selected.current = Some(Arc::new(View::Template(loaded)));
                selected.phase = "conflict";
                selected.rebuild()?;
                let next = Id::new();
                let selection = selected.dto(next);
                let owned = registry
                    .selections
                    .remove(snapshot)
                    .ok_or(Code::UnknownId)?;
                registry.selections.insert(next, owned);
                return Ok(Completed::new(
                    (),
                    Ok(ResultDto::RecoverySelection { selection }),
                ));
            }
            let choices = reapply.as_ref().ok_or(Code::InvalidInput)?;
            let mapped_choices = choices
                .iter()
                .map(|choice| match choice {
                    ReapplyIntent::Name => ReapplyIntent::Change {
                        change: "[\"name\"]".into(),
                    },
                    other => other.clone(),
                })
                .collect::<Vec<_>>();
            let choices = &mapped_choices;
            if choices.iter().any(|i| !selected.intents.contains(i)) {
                return Err(Code::InvalidInput.into());
            }
            let ids = choices
                .iter()
                .map(|choice| match choice {
                    ReapplyIntent::Change { change } => Ok(change.clone()),
                    _ => Err(Code::InvalidInput.into()),
                })
                .collect::<Reply<Vec<_>>>()?;
            body = serde_json::from_value(
                selected
                    .comparison
                    .as_ref()
                    .ok_or(Code::WrongBinding)?
                    .apply(&ids)
                    .map_err(|_| Code::WrongBinding)?,
            )
            .map_err(|_| Code::InvalidInput)?;
            super::recovery_template::restoration_intents(&mut body, loaded.artifact())?;
        }
        Some(Arc::new(View::Template(loaded)))
    } else {
        if reapply.is_some() {
            return Err(Code::InvalidInput.into());
        }
        None
    };
    if envelope.attempt.as_ref().is_some_and(|a| {
        matches!(
            a.result,
            crate::data::edit_recovery::model::SaveState::Unknown
                | crate::data::edit_recovery::model::SaveState::Uncertain
        )
    }) {
        selected.phase = "uncertain";
        return Ok(Completed::new(
            (),
            Ok(ResultDto::RecoverySelection {
                selection: selected.dto(*snapshot),
            }),
        ));
    }
    let draft_id = envelope.key.draft_id.clone();
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
    let id = match &base {
        Some(v) => v.template()?.id,
        None => seed.as_ref().ok_or(Code::WrongBinding)?.template_id(),
    };
    let target = ArtifactSourceId::Template(id)
        .path()
        .map_err(|_| Code::InvalidInput)?;
    let unpublished = base.is_none();
    let mut entry = Entry {
        resume: None,
        comparison_checkpoint: None,
        residual: None,
        residual_ack: None,
        resumed_attempt: None,
        owner: job.allocated,
        project: *project,
        fingerprint: envelope.key.project_fingerprint.clone(),
        draft_id,
        base,
        seed,
        body,
        generation,
        saved_generation: None,
        snapshot: Id::new(),
        content: String::new(),
        phase: "editing",
        receipt: None,
        error: None,
        problems: vec![],
        outcome: None,
        submitted: None,
        deposit: None,
        original: None,
        expected_digest: None,
        restore_assets: Some(ReceiptDto {
            key: envelope.key.clone(),
            deposit_id: envelope.deposit_id.clone(),
            digest: selected.deposit.payload_digest().into(),
        }),
        restored_from: Some(envelope.key),
        proof: None,
        release_first: None,
        release_latest: None,
        handoff_receipt: None,
        handoff_failure: None,
        discarded_input: None,
    };
    // 변환/직렬화 실패는 worker 등록 전에 끝낸다. 등록 후 부분 acquire 실패는
    // 기존 SessionRegistration과 Entry가 소유하며 명시적 release까지 유지한다.
    entry.rebuild_content()?;
    let mut status = entry.status()?;
    let (binding, registration) = operations::begin_owner(ctx, job, vec![target], unpublished)?;
    entry.error = registration
        .original
        .as_ref()
        .err()
        .map(|_| Code::SessionRejected.into());
    status.error = entry.error.clone();
    entry.original = Some(Box::new(registration));
    let mut result = Completed::new(
        (),
        Ok(ResultDto::TemplateDraft {
            status: Box::new(status),
        }),
    );
    result.binding = Some(binding);
    registry.entries.insert(entry.owner, entry);
    registry.owners.store(
        registry.entries.len() + registry.documents.len(),
        std::sync::atomic::Ordering::Release,
    );
    Ok(result)
}
#[cfg(test)]
fn offered(body: &TemplateBody, current: &TemplateArtifact) -> Reply<Vec<ReapplyIntent>> {
    let mut out = vec![ReapplyIntent::Name, ReapplyIntent::Presentation];
    let now = body_from_source(current)?;
    for field in &body.fields {
        if now.fields.iter().any(|f| {
            f.id == field.id
                && !f.archived
                && std::mem::discriminant(&f.configuration)
                    == std::mem::discriminant(&field.configuration)
        }) {
            out.extend([
                ReapplyIntent::FieldLabel {
                    field: field.id.clone(),
                },
                ReapplyIntent::FieldRequired {
                    field: field.id.clone(),
                },
                ReapplyIntent::FieldWritingGuide {
                    field: field.id.clone(),
                },
                ReapplyIntent::FieldPresentation {
                    field: field.id.clone(),
                },
                ReapplyIntent::FieldDefault {
                    field: field.id.clone(),
                },
            ]);
            let target = now
                .fields
                .iter()
                .find(|f| f.id == field.id)
                .ok_or(Code::WrongBinding)?;
            if title_reapply(&field.configuration, &target.configuration).is_some() {
                out.push(ReapplyIntent::FieldCardTitle {
                    field: field.id.clone(),
                });
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
fn apply_selected(
    body: &TemplateBody,
    current: &TemplateArtifact,
    choices: &[ReapplyIntent],
) -> Reply<TemplateBody> {
    let mut draft = body_from_source(current)?;
    for choice in choices {
        let id = match choice {
            ReapplyIntent::Change { .. } | ReapplyIntent::ComponentTemplate => {
                return Err(Code::InvalidInput.into())
            }
            ReapplyIntent::Name => {
                draft.name = body.name.clone();
                continue;
            }
            ReapplyIntent::Presentation => {
                draft.presentation = body.presentation.clone();
                continue;
            }
            ReapplyIntent::FieldLabel { field }
            | ReapplyIntent::FieldRequired { field }
            | ReapplyIntent::FieldWritingGuide { field }
            | ReapplyIntent::FieldPresentation { field }
            | ReapplyIntent::FieldCardTitle { field }
            | ReapplyIntent::FieldDefault { field } => field,
        };
        let from = body
            .fields
            .iter()
            .find(|f| &f.id == id)
            .ok_or(Code::WrongBinding)?;
        let to = draft
            .fields
            .iter_mut()
            .find(|f| &f.id == id)
            .ok_or(Code::WrongBinding)?;
        match choice {
            ReapplyIntent::FieldLabel { .. } => to.label = from.label.clone(),
            ReapplyIntent::FieldRequired { .. } => to.required = from.required,
            ReapplyIntent::FieldWritingGuide { .. } => {
                to.writing_guide = from.writing_guide.clone()
            }
            ReapplyIntent::FieldPresentation { .. } => to.presentation = from.presentation.clone(),
            ReapplyIntent::FieldCardTitle { .. } => {
                let intent = title_reapply(&from.configuration, &to.configuration)
                    .ok_or(Code::WrongBinding)?;
                if let DraftConfiguration::Group {
                    card_title_field, ..
                } = &mut to.configuration
                {
                    *card_title_field = intent;
                }
            }
            ReapplyIntent::FieldDefault { .. } => to.default = from.default.clone(),
            _ => return Err(Code::InvalidInput.into()),
        }
    }
    // 재적용은 새 초안일 뿐이다. 전체 유효성 검증과 쓰기는 사용자의 다음 저장에 남는다.
    Ok(draft)
}

#[cfg(test)]
fn title_reapply(from: &DraftConfiguration, to: &DraftConfiguration) -> Option<Intent<String>> {
    match (from, to) {
        (
            DraftConfiguration::Group {
                card_title_field: Intent::Unset,
                ..
            },
            DraftConfiguration::Group { .. },
        ) => Some(Intent::Unset),
        (
            DraftConfiguration::Group {
                card_title_field: Intent::Set(id),
                ..
            },
            DraftConfiguration::Group { members, .. },
        ) if members.iter().any(|f| {
            f.id == *id && !f.archived && matches!(f.configuration, DraftConfiguration::RichText {})
        }) =>
        {
            Some(Intent::Set(id.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod card_title_tests {
    use super::*;
    #[test]
    fn card_title_reapply_accepts_only_existing_active_rich_member_and_explicit_unset() {
        let id = "33333333-3333-4333-8333-333333333333";
        let member: DraftField = serde_json::from_value(serde_json::json!({"id":id,"label":"title","configuration":{"kind":"rich_text"},"required":false,"presentation":{"intent":"keep"},"default":{"intent":"keep"},"archived":false})).unwrap();
        let from = DraftConfiguration::Group {
            members: vec![],
            card_title_field: Intent::Set(id.into()),
        };
        let mut to = DraftConfiguration::Group {
            members: vec![member],
            card_title_field: Intent::Keep,
        };
        assert!(title_reapply(&from, &to) == Some(Intent::Set(id.into())));
        if let DraftConfiguration::Group { members, .. } = &mut to {
            members[0].archived = true;
        }
        assert!(title_reapply(&from, &to).is_none());
        assert!(matches!(
            title_reapply(
                &DraftConfiguration::Group {
                    members: vec![],
                    card_title_field: Intent::Unset
                },
                &to
            ),
            Some(Intent::Unset)
        ));
    }
}
