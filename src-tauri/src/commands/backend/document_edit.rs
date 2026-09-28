//! 기존 문서의 원본·raw·세대·G10 결과를 화면과 독립적으로 소유한다.
use super::document_workspace::{finish_session, generation, observed, reply};
use super::*;
use crate::commands::document_workspace::{EditBody, ReadField, Request, Response};
use crate::data::edit_recovery::{
    error::Category,
    model::{Attempt, Deposit, Draft, Envelope, Intent, Key, OriginalKind},
    Proof,
};
use crate::data::field_engine::scalar::validate_optional_single_line_text;
use crate::data::repository::DocumentScanChange;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

#[derive(Default)]
pub(super) struct Registry {
    entries: BTreeMap<Id, Entry>,
}
impl Registry {
    pub(super) fn owns_document(&self, project: Id, document: artifact::DocumentId) -> bool {
        self.entries.values().any(|entry| {
            entry.project == project
                && entry
                    .document
                    .document()
                    .is_ok_and(|source| source.id == document)
        })
    }

    pub(super) fn asset_owner(
        &self,
        project: Id,
        owner: Id,
        generation: &str,
        field: &str,
        image: bool,
        cell: Option<&artifact::group::CellAddress>,
    ) -> bool {
        self.entries.get(&owner).is_some_and(|e| {
            e.project == project
                && e.generation.to_string() == generation
                && e.editable.iter().any(|f| f == field)
                && match &*e.template {
                    View::Template(t) if cell.is_some() => match (&*e.document, cell) {
                        (View::Document(d), Some(cell)) => artifact::group::asset_target(
                            t.artifact(),
                            &e.body.fields,
                            Some(d.artifact()),
                            field,
                            cell,
                            image,
                        ),
                        _ => false,
                    },
                    View::Template(t) => field
                        .parse()
                        .ok()
                        .and_then(|id| t.artifact().fields().get(&id))
                        .is_some_and(|f| {
                            f.kind()
                                == if image {
                                    artifact::FieldKind::Image
                                } else {
                                    artifact::FieldKind::File
                                }
                        }),
                    _ => false,
                }
        })
    }

    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
    pub(super) fn retains(&self, key: &Key) -> bool {
        self.entries
            .values()
            .any(|e| e.fingerprint == key.project_fingerprint && e.draft == key.draft_id)
    }
}
struct Entry {
    project: Id,
    fingerprint: String,
    draft: String,
    generation: u64,
    saved: Option<u64>,
    document: Arc<View>,
    template: Arc<View>,
    binding: Binding,
    body: EditBody,
    read: Response,
    editable: Vec<String>,
    deposit: Option<Deposit>,
    proof: Option<Proof>,
    attempt: Option<Attempt>,
    submitted: Option<Envelope>,
    outcome: Option<Box<ResultDto>>,
    expected: Option<String>,
    problem: Option<String>,
    field: Option<String>,
    observations: Vec<Box<dyn Any + Send>>,
    // commit은 끝났지만 candidate 재독이 남은 동안 이전 source와 attempt를 잃지 않는다.
    pending_scan_change: Option<PendingScanChange>,
    // 성공한 현재 세대의 canonical transition만 workspace scan cache에 전달한다.
    scan_change: Option<(u64, DocumentScanChange)>,
}
#[derive(Clone)]
struct PendingScanChange {
    generation: u64,
    operation_id: String,
    previous: crate::data::repository::SourceToken,
    candidate_digest: String,
}
impl Entry {
    fn response(&self, owner: Id) -> Reply<Completed> {
        Ok(reply(Response::Editing {
            owner,
            document: self.document.document()?.id.to_string(),
            generation: self.generation.to_string(),
            saved_generation: self.saved.map(|g| g.to_string()),
            body: self.body.clone(),
            read: Box::new(self.read.clone()),
            editable: self.editable.clone(),
            source: source_digest(&self.document)?,
            deposited: self.deposited(),
            outcome: self.outcome.clone(),
            problem: self.problem.clone(),
            field: self.field.clone(),
        }))
    }
    fn deposited(&self) -> bool {
        self.deposit
            .as_ref()
            .zip(self.proof.as_ref())
            .is_some_and(|(d, p)| p.matches(d.key(), &d.envelope().deposit_id, d.payload_digest()))
    }
    fn update(&mut self, raw: &str, body: &EditBody) -> Reply<()> {
        let g = generation(raw)?;
        if g < self.generation || (g == self.generation && body != &self.body) {
            return Err(Code::WrongBinding.into());
        }
        if g != self.generation {
            self.deposit = None;
            self.proof = None;
            // 새 raw 세대는 이전 보관 receipt를 이어받지 않는다. 다만 이미 내구
            // commit된 source 전이는 UI 초안 세대와 수명이 다르므로, 정확한
            // candidate 재독으로 종결될 때까지 원 attempt에 묶어 유지한다.
            self.scan_change = None;
        }
        self.generation = g;
        self.body = body.clone();
        self.field = None;
        Ok(())
    }
    fn envelope(&self) -> Reply<Envelope> {
        Ok(Envelope {
            key: Key {
                project_fingerprint: self.fingerprint.clone(),
                draft_id: self.draft.clone(),
                generation: self.generation,
            },
            deposit_id: Id::new().into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            created_at_utc: timestamp()?,
            originals: [&self.template, &self.document]
                .iter()
                .map(|v| recovery::original(v).map_err(|_| Code::RecoveryRejected.into()))
                .collect::<Reply<_>>()?,
            draft: Draft::Document {
                document: Some(self.document.document()?.id.to_string()),
                template: self.template.template()?.id.to_string(),
                name: self.body.name.clone(),
                english_name: self.body.english_name.clone(),
                glossary_summary: self.body.glossary_summary.clone(),
                glossary_excluded: self.body.glossary_excluded.clone(),
                fields: self.body.fields.clone(),
                composing: self.body.composing,
            },
            attempt: self.attempt.clone(),
        })
    }
}
fn source_digest(v: &View) -> Reply<String> {
    Ok(match v {
        View::Document(d) => d.source(),
        View::Template(t) => t.source(),
    }
    .sha256()
    .iter()
    .map(|b| format!("{b:02x}"))
    .collect())
}
fn sha256_bytes(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0_u8; 32];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(result)
}
fn load(
    ctx: &mut Context,
    id: artifact::DocumentId,
    notes: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<(Arc<View>, Arc<View>)> {
    let (d, t) = ctx
        .read(|r| {
            let repo = ArtifactRepository::new(r)?;
            let d = repo.load_document(id)?;
            let t = repo.load_template(d.artifact().template_id())?;
            Ok::<_, crate::data::repository::RepositoryError>((d, t))
        })
        .map_err(|e| observed(notes, e, Code::RuntimeRejected))?
        .map_err(|e| observed(notes, e, Code::RepositoryRejected))?;
    if t.artifact().lifecycle() != artifact::TemplateLifecycle::Active {
        return Err(Code::WrongBinding.into());
    }
    // layout은 정확한 대상 상태만 확인하며 전체 문서를 scan하지 않는다.
    let layout = ctx
        .read(|r| ArtifactRepository::new(r)?.load_layout())
        .map_err(|e| observed(notes, e, Code::RuntimeRejected))?
        .map_err(|e| observed(notes, e, Code::RepositoryRejected))?;
    if layout.as_ref().is_some_and(|l| {
        l.artifact()
            .nodes
            .get(&id)
            .is_some_and(|n| n.state == artifact::layout::LayoutState::Trashed)
    }) {
        return Err(Code::WrongBinding.into());
    }
    Ok((Arc::new(View::Document(d)), Arc::new(View::Template(t))))
}
fn project(d: &View, t: &View) -> Reply<(Response, Vec<String>)> {
    let View::Document(d) = d else {
        return Err(Code::WrongBinding.into());
    };
    let View::Template(t) = t else {
        return Err(Code::WrongBinding.into());
    };
    let r = artifact::reconcile_document(t.artifact(), d.artifact())
        .map_err(|_| Code::RepositoryRejected)?;
    let field_problems = r
        .warnings()
        .iter()
        .map(|warning| (warning.field_id(), format!("{:?}", warning.category())))
        .chain(
            r.blocking_issues()
                .iter()
                .map(|issue| (issue.field_id(), format!("{:?}", issue.category()))),
        )
        .collect::<BTreeMap<_, _>>();
    let mut editable = vec![];
    let mut fields = vec![];
    for f in r.known_fields() {
        let id = f.field_id().to_string();
        let active = format!("{:?}", f.lifecycle()) == "Active";
        // 검증된 known AST는 서식까지 편집하며 unknown 원본은 Keep으로 보존한다.
        let supported = f
            .value()
            .and_then(|v| v.rich_text())
            .is_none_or(|v| !v.contains_unknown_storage_data());
        if active && supported {
            editable.push(id.clone());
        }
        fields.push(ReadField {
            id,
            label: f.label().into(),
            state: format!("{:?}", f.lifecycle()),
            provenance: f.provenance().map(|p| format!("{p:?}")),
            value: f
                .value()
                .map(|v| projection::field_value(v, t.artifact().fields().get(&f.field_id())))
                .transpose()?,
            problem: field_problems.get(&f.field_id()).cloned(),
        });
    }
    for f in r.orphan_fields() {
        fields.push(ReadField {
            id: f.field_id().to_string(),
            label: f
                .display_label()
                .map(str::to_owned)
                .unwrap_or_else(|| f.field_id().to_string()),
            state: "Orphan".into(),
            provenance: Some("ExistingValue".into()),
            value: Some(projection::value(f.value())?),
            problem: None,
        });
    }
    Ok((
        Response::Read {
            schema: d.artifact().schema_version().get(),
            id: d.artifact().document_id().to_string(),
            name: d.artifact().name().into(),
            english_name: d.artifact().english_name().into(),
            glossary_summary: d.artifact().glossary_summary().into(),
            glossary_excluded: d.artifact().glossary_excluded(),
            template: projection::template(t.artifact())?,
            fields,
            warnings: r
                .warnings()
                .iter()
                .map(|w| format!("{:?}", w.category()))
                .chain(
                    r.blocking_issues()
                        .iter()
                        .map(|w| format!("{:?}", w.category())),
                )
                .collect(),
        },
        editable,
    ))
}
fn begin_entry(
    ctx: &mut Context,
    job: &Job,
    project_id: Id,
    id: artifact::DocumentId,
) -> Reply<Entry> {
    // The app-owned recovery store must be held before asking the SVN server for a lock.
    // Store::open's exclusive handle stays in Owner for this process's lifetime.
    if job.collaborative {
        job.recovery.connect().map_err(recovery_admission)?;
    }
    let mut notes = vec![];
    let (mut document, mut template) = load(ctx, id, &mut notes)?;
    let target = ArtifactSourceId::Document(id)
        .path()
        .map_err(|_| Code::InvalidInput)?;
    let (binding, registration) = begin(ctx, job, vec![target])?;
    let mut problem = registration
        .original
        .as_ref()
        .err()
        .map(|_| "SessionRejected".into());
    if problem.is_none()
        && job.collaborative
        && !binding
            .recovery
            .connected
            .load(std::sync::atomic::Ordering::Acquire)
    {
        problem = Some("RecoveryUnavailable".into());
    }
    notes.push(Box::new(registration));

    // 현재 revision의 선택형 반복 그룹 key가 빠진 경우에는 편집을 시작하는 이
    // 세션에서 canonical 빈 목록을 먼저 내구 저장한다. 필수/스칼라/과거 revision
    // 누락은 이 경로에서 추론하지 않는다. 저장이 확정되면 편집 snapshot도 반드시
    // 새 source를 다시 읽어 이후 저장이 stale source로 거절되지 않게 한다.
    // 오래된 Template revision에 묶인 문서는 일반 저장 경로에서 현재 revision으로
    // 승격한다. 빈 그룹 보정은 현재 revision 전용이므로 여기서 호출하면 정상적인
    // 과거 문서까지 OptionalGroupRepairRejected로 편집을 막게 된다.
    let current_revision = match (&*document, &*template) {
        (View::Document(d), View::Template(t)) => {
            d.artifact().template_revision() == t.artifact().revision()
        }
        _ => return Err(Code::WrongBinding.into()),
    };
    if problem.is_none() && current_revision {
        let input = persistence::MaterializeDocumentInput {
            document: document.document()?,
            template: template.template()?,
            timestamp_utc: timestamp()?,
        };
        let repair = binding
            .key
            .as_ref()
            .ok_or(Code::SessionRejected)
            .and_then(|key| {
                ctx.session(key)
                    .map_err(|_| Code::SessionRejected)
                    .and_then(|mut session| {
                        session
                            .repair_missing_optional_group_values(&input)
                            .map_err(|_| Code::SaveRejected)
                    })
            });
        match repair {
            Ok(execution) => {
                let changed = execution.outcome().is_some_and(|outcome| {
                    outcome.kind() == artifact::DocumentMaterializationOutcomeKind::Changed
                });
                let diagnostic = execution.execution.diagnostic();
                let committed = diagnostic.category.is_none()
                    && matches!(
                        diagnostic.disk,
                        crate::data::application::diagnostics::DiskState::Committed
                            | crate::data::application::diagnostics::DiskState::NoWrite
                    );
                notes.push(Box::new(execution));
                if committed && changed {
                    (document, template) = load(ctx, id, &mut notes)?;
                } else if !committed {
                    problem = Some("OptionalGroupRepairRejected".into());
                }
            }
            Err(error) => {
                notes.push(Box::new(error));
                problem = Some("OptionalGroupRepairRejected".into());
            }
        }
    }
    let (read, editable) = project(&document, &template)?;
    Ok(Entry {
        project: project_id,
        fingerprint: ctx.project_fingerprint().ok_or(Code::Unavailable)?.into(),
        draft: job.allocated.into(),
        generation: 1,
        saved: Some(1),
        document,
        template,
        binding,
        body: EditBody {
            name: Intent::Keep,
            english_name: Intent::Keep,
            glossary_summary: Intent::Keep,
            glossary_excluded: Intent::Keep,
            fields: vec![],
            composing: false,
        },
        read,
        editable,
        deposit: None,
        proof: None,
        attempt: None,
        submitted: None,
        outcome: None,
        expected: None,
        problem,
        field: None,
        observations: notes,
        pending_scan_change: None,
        scan_change: None,
    })
}
fn edits(e: &mut Entry) -> Reply<Vec<DocumentEdit>> {
    let mut edits = vec![];
    match &e.body.name {
        Intent::Keep => (),
        Intent::Set(name) if !name.trim().is_empty() => {
            edits.push(DocumentEdit::Rename { name: name.clone() })
        }
        _ => {
            e.problem = Some("NameRequired".into());
            return Err(Code::InvalidInput.into());
        }
    }
    match &e.body.english_name {
        Intent::Keep => (),
        Intent::Set(value) if validate_optional_single_line_text(value).is_ok() => {
            edits.push(DocumentEdit::EnglishName {
                value: value.clone(),
            })
        }
        Intent::Unset => edits.push(DocumentEdit::EnglishName {
            value: String::new(),
        }),
        Intent::Set(_) => {
            e.problem = Some("SingleLineRequired".into());
            e.field = Some("englishName".into());
            return Err(Code::InvalidInput.into());
        }
    }
    match &e.body.glossary_summary {
        Intent::Keep => (),
        Intent::Set(value) if validate_optional_single_line_text(value).is_ok() => {
            edits.push(DocumentEdit::GlossarySummary {
                value: value.clone(),
            })
        }
        Intent::Unset => edits.push(DocumentEdit::GlossarySummary {
            value: String::new(),
        }),
        Intent::Set(_) => {
            e.problem = Some("SingleLineRequired".into());
            e.field = Some("glossarySummary".into());
            return Err(Code::InvalidInput.into());
        }
    }
    if let Intent::Set(excluded) = &e.body.glossary_excluded {
        edits.push(DocumentEdit::GlossaryExcluded {
            excluded: *excluded,
        });
    } else if matches!(e.body.glossary_excluded, Intent::Unset) {
        return Err(Code::InvalidInput.into());
    }
    let mut seen = BTreeSet::new();
    for f in &e.body.fields {
        if !seen.insert(&f.field) {
            return Err(Code::InvalidInput.into());
        }
        if matches!(f.value, Intent::Keep) {
            continue;
        }
        if !e.editable.contains(&f.field) {
            e.field = Some(f.field.clone());
            e.problem = Some("ReadOnlyValue".into());
            return Err(Code::InvalidInput.into());
        }
        edits.push(match &f.value {
            Intent::Keep => continue,
            Intent::Unset => DocumentEdit::Unset {
                field: f.field.clone(),
            },
            Intent::Set(v @ ValueDto::Group { instances }) => {
                let View::Template(t) = &*e.template else {
                    return Err(Code::WrongBinding.into());
                };
                let View::Document(d) = &*e.document else {
                    return Err(Code::WrongBinding.into());
                };
                let id = convert::id(&f.field)?;
                let definition = t.artifact().fields().get(&id).ok_or(Code::InvalidInput)?;
                let inputs = convert::group_inputs(instances)?;
                if let Err(error) = artifact::group::assemble(
                    definition,
                    t.artifact().revision(),
                    d.artifact().field_values().get(&id),
                    &inputs,
                ) {
                    e.problem = Some(error.category.into());
                    e.field = Some(super::document_workspace::group_error_address(
                        &f.field, &error,
                    ));
                    return Err(Code::InvalidInput.into());
                }
                DocumentEdit::Set {
                    field: f.field.clone(),
                    value: v.clone(),
                }
            }
            Intent::Set(v) => {
                let value = v.creation_value().map_err(|_| {
                    e.field = Some(f.field.clone());
                    e.problem = Some("InvalidValue".into());
                    Code::InvalidInput
                })?;
                if matches!(projection::value(&value)?, ValueDto::Unset {}) {
                    DocumentEdit::Unset {
                        field: f.field.clone(),
                    }
                } else {
                    DocumentEdit::Set {
                        field: f.field.clone(),
                        value: projection::value(&value)?,
                    }
                }
            }
        });
    }
    // SetGroup을 거치지 않는 이름/다른 필드 저장에도 같은 유효값과 정확한 셀 주소를 적용한다.
    if let (View::Template(t), View::Document(d)) = (&*e.template, &*e.document) {
        for (id, definition) in t.artifact().fields() {
            if definition.lifecycle() != artifact::FieldLifecycle::Active
                || e.body
                    .fields
                    .iter()
                    .any(|f| f.field == id.to_string() && !matches!(f.value, Intent::Keep))
            {
                continue;
            }
            if let (Some((_, members)), Some(value)) = (
                definition.configuration().members(),
                d.artifact().field_values().get(id),
            ) {
                if let Err((error, _)) = artifact::group::validate_cells(members, value, crate::data::field_engine::validation::BoundDocumentValueContext::ExistingDocumentValue) {
                    e.problem = Some(error.category.into());
                    e.field = Some(super::document_workspace::group_error_address(&id.to_string(), &error));
                    return Err(Code::InvalidInput.into());
                }
            }
        }
    }
    Ok(edits)
}
fn refresh(ctx: &mut Context, job: &Job, e: &mut Entry) -> Reply<()> {
    let expected = e.expected.as_ref().ok_or(Code::WrongBinding)?.clone();
    let (d, t) = load(ctx, e.document.document()?.id, &mut e.observations)?;
    if source_digest(&d)? != expected || source_digest(&t)? != source_digest(&e.template)? {
        return Err(Code::WrongBinding.into());
    }
    let (read, editable) = project(&d, &t)?;
    // 명시적 runtime 복구가 끝나도 이전 저장의 pending P는 남을 수 있다.
    // 실제 sink proof/ack를 먼저 얻어야 다음 raw를 bind할 수 있으며, 읽기 성공만으로 버리지 않는다.
    let previous = ctx
        .session_control()
        .deposit_whole_draft(&e.binding.registration);
    if !matches!(&previous, Ok(Ok(_))) {
        return Err(observed(
            &mut e.observations,
            previous,
            Code::RecoveryRejected,
        ));
    }
    e.observations.push(Box::new(previous));
    let session = ctx
        .session_control()
        .observe_session(&e.binding.registration)
        .map_err(|error| observed(&mut e.observations, error, Code::SessionRejected))?;
    let needs_session = session.0.state() != crate::data::edit_session::EditSessionState::Editing;
    e.observations.push(Box::new(session));
    if needs_session || e.binding.key.is_none() {
        let (released, note) = finish_session(ctx, &e.binding);
        e.observations.push(note);
        if !released {
            return Err(Code::ReleaseRejected.into());
        }
        let target = d.target().path().map_err(|_| Code::InvalidInput)?;
        let (mut binding, registration) = begin(ctx, job, vec![target])?;
        binding.id = e.binding.id;
        let acquired = binding.key.is_some();
        e.binding = binding;
        e.observations.push(Box::new(registration));
        if !acquired {
            return Err(Code::SessionRejected.into());
        }
    }
    confirm_scan_change(e, &d)?;
    e.document = d;
    e.template = t;
    e.read = read;
    e.editable = editable;
    e.expected = None;
    e.problem = None;
    Ok(())
}

/// 즉시 save-refresh와 복구 뒤 지연 refresh가 같은 commit 증거를 종결한다.
/// 현재 raw 세대가 앞서도 원 attempt·저장 세대·candidate digest를 독립 검증한다.
fn confirm_scan_change(e: &mut Entry, current: &Arc<View>) -> Reply<()> {
    let Some(pending) = e.pending_scan_change.clone() else {
        return Ok(());
    };
    let Some(attempt) = e.attempt.as_ref() else {
        return Ok(());
    };
    if e.generation < pending.generation
        || e.saved != Some(pending.generation)
        || attempt.submitted_generation != pending.generation
        || attempt.operation_id != pending.operation_id
        || attempt.result != crate::data::edit_recovery::model::SaveState::Committed
        || attempt.candidate_digest.as_deref() != Some(pending.candidate_digest.as_str())
        || source_digest(current)? != pending.candidate_digest
    {
        return Ok(());
    }
    let View::Document(current) = &**current else {
        return Err(Code::WrongBinding.into());
    };
    if let Some(change) = DocumentScanChange::committed(pending.previous, current) {
        // 전달 세대는 현재 응답에 묶되, change 자체의 권한은 위 원 commit 증거다.
        e.scan_change = Some((e.generation, change));
        e.pending_scan_change = None;
    }
    Ok(())
}
fn save(ctx: &mut Context, job: &Job, e: &mut Entry) -> Reply<()> {
    // An already open editor can outlive a failed backend binding. Recheck at the
    // actual write boundary, before binding a new draft or changing canonical bytes.
    if job.collaborative {
        job.recovery.connect().map_err(recovery_admission)?;
        recovery::connect_checked(&mut ctx.session_control(), job, &e.binding)
            .map_err(recovery_admission)?;
    }
    // 이미 freeze한 세대의 attempt 증거를 다른 저장 시도로 바꾸지 않는다.
    if e.deposit.is_some() {
        return Err(Code::OwnersRemain.into());
    }
    if e.saved == Some(e.generation) && e.submitted.is_some() {
        return Ok(());
    }
    if e.expected.is_some()
        || e.problem.as_deref().is_some_and(|p| {
            [
                "Uncertain",
                "SourceChanged",
                "SaveFailed",
                "SessionRejected",
                "SavedReadRequired",
            ]
            .contains(&p)
        })
    {
        return Err(Code::OwnersRemain.into());
    }
    if e.saved.is_some() && e.problem.is_none() && e.expected.is_none() {
        e.observations.clear();
    }
    e.problem = None;
    e.field = None;
    if e.body.composing {
        e.problem = Some("Composing".into());
        return Ok(());
    }
    let View::Document(current_document) = &*e.document else {
        return Err(Code::WrongBinding.into());
    };
    let View::Template(current_template) = &*e.template else {
        return Err(Code::WrongBinding.into());
    };
    if let Some((problem, field)) = super::document_workspace::validate_reference_inputs(
        ctx,
        current_template.artifact(),
        current_document.artifact().document_id(),
        &e.body.fields,
        &mut e.observations,
    )? {
        e.problem = Some(problem);
        e.field = Some(field);
        return Ok(());
    }
    let raw = match edits(e) {
        Ok(v) => v,
        Err(error) => {
            e.problem.get_or_insert_with(|| "InvalidValue".into());
            e.observations.push(Box::new(error));
            return Ok(());
        }
    };
    let converted = match convert::edits(&raw) {
        Ok(v) => v,
        Err(error) => {
            e.problem = Some("InvalidValue".into());
            e.observations.push(Box::new(error));
            return Ok(());
        }
    };
    let input = persistence::SaveDocumentInput {
        document: e.document.document()?,
        template: e.template.template()?,
        edits: converted,
        timestamp_utc: timestamp()?,
    };
    // 순수 검증은 실제 저장 전에 수행하여 invalid raw의 오류 위치를 남긴다.
    let View::Document(d) = &*e.document else {
        return Err(Code::WrongBinding.into());
    };
    let previous_source = d.source().clone();
    let View::Template(t) = &*e.template else {
        return Err(Code::WrongBinding.into());
    };
    if let Err(error) = artifact::prepare_document_save(
        t.artifact(),
        input.template.expected_revision,
        d.artifact(),
        &input.edits,
        &input.timestamp_utc,
    ) {
        e.problem = Some(format!("{:?}", error.category()));
        e.field = error.field_id().map(|id| id.to_string());
        e.observations.push(Box::new(error));
        return Ok(());
    }
    let mut envelope = e.envelope()?;
    let mut attempt = recovery::attempt(job.operation, None);
    attempt.submitted_generation = e.generation;
    envelope.attempt = Some(attempt);
    let frozen = Deposit::freeze(envelope.clone())
        .map_err(|error| observed(&mut e.observations, error, Code::RecoveryRejected))?;
    recovery::capture_assets(ctx, job, &frozen, &mut e.observations)?;
    let attempt_cell = Arc::new(OnceLock::new());
    let payload = PendingEdit {
        input: job.input.clone(),
        views: vec![e.template.clone(), e.document.clone()],
        recovery: recovery::PendingRecovery::whole(envelope.clone(), attempt_cell.clone()),
    };
    let key = ctx
        .session_key(&e.binding.registration)
        .ok_or(Code::SessionRejected)?;
    recovery::connect(&mut ctx.session_control(), job, &e.binding);
    let previous = ctx
        .session(&key)
        .map_err(|_| Code::SessionRejected)?
        .bind_whole_draft(payload, |old, new| {
            old.recovery.envelope.key.draft_id == e.draft
                && new.recovery.envelope.key.draft_id == e.draft
                && old.recovery.envelope.key.generation < new.recovery.envelope.key.generation
                && old.recovery.envelope.originals == new.recovery.envelope.originals
        })
        .map_err(|error| observed(&mut e.observations, error, Code::OwnersRemain))?;
    let (result, resolved) = ctx
        .session(&key)
        .map_err(|_| Code::SessionRejected)?
        .write_whole_document(&input);
    let execution = result.as_ref().ok().and_then(|r| r.original.as_ref().ok());
    let diagnostic = execution.map(|r| r.execution.diagnostic());
    let mut attempt = recovery::attempt_ref(job.operation, diagnostic.as_ref());
    attempt.submitted_generation = e.generation;
    attempt.candidate_digest = execution
        .and_then(|r| r.outcome())
        .map(|o| artifact::encode_document(o.document()))
        .transpose()
        .map_err(|error| observed(&mut e.observations, error, Code::SerializationFailed))?
        .map(|v| crate::data::edit_recovery::model::digest(&v));
    attempt_cell
        .set(attempt.clone())
        .map_err(|_| Code::DuplicateConflict)?;
    envelope.attempt = Some(attempt.clone());
    e.submitted = Some(envelope);
    e.attempt = Some(attempt.clone());
    if let Some(diagnostic) = diagnostic {
        let disk = diagnostic.disk;
        let mut outcome = write(
            e.binding.id,
            Some(input.document.id.to_string()),
            diagnostic,
            None,
            vec![],
        );
        if let ResultDto::Write { diagnostic, .. } = &mut outcome {
            diagnostic.operation_id = Some(job.operation);
            diagnostic.observed_at_utc = Some(input.timestamp_utc.clone());
        }
        e.outcome = Some(Box::new(outcome));
        match disk {
            DiskState::Committed | DiskState::NoWrite => {
                e.saved = Some(e.generation);
                e.expected = if disk == DiskState::NoWrite {
                    Some(source_digest(&e.document)?)
                } else {
                    attempt.candidate_digest.clone()
                };
                if disk == DiskState::Committed {
                    e.pending_scan_change =
                        attempt
                            .candidate_digest
                            .clone()
                            .map(|digest| PendingScanChange {
                                generation: e.generation,
                                operation_id: attempt.operation_id.clone(),
                                previous: previous_source,
                                candidate_digest: digest,
                            });
                }
                e.problem = Some("SavedReadRequired".into());
                if let Err(error) = refresh(ctx, job, e) {
                    e.observations.push(Box::new(error));
                }
            }
            DiskState::Uncertain => e.problem = Some("Uncertain".into()),
            // rollback/미적용은 원본 변경이나 권한 실패로 단정할 수 없다.
            _ => e.problem = Some("SaveFailed".into()),
        }
    } else {
        e.problem = Some("SessionRejected".into());
    }
    e.observations
        .push(Box::new((input, result, resolved, previous)));
    Ok(())
}
fn recovery_admission(
    error: std::sync::Arc<crate::data::edit_recovery::error::RecoveryError>,
) -> crate::commands::dto::ErrorDto {
    if error.category == Category::Busy {
        Code::RecoveryStoreBusy.into()
    } else {
        Code::SinkUnavailable.into()
    }
}
pub(super) fn committed_change(
    registry: &Registry,
    project: Id,
    request: &Request,
    restored_owner: Id,
) -> Option<DocumentScanChange> {
    let (owner, requested_generation) = match request {
        Request::EditDraft {
            owner,
            generation,
            save: true,
            ..
        } => (*owner, Some(generation.parse::<u64>().ok()?)),
        Request::EditRefresh { owner } => (*owner, None),
        Request::EditRestore { .. } => (restored_owner, None),
        _ => return None,
    };
    let entry = registry.entries.get(&owner)?;
    let generation = requested_generation.unwrap_or(entry.generation);
    let (change_generation, change) = entry.scan_change.as_ref()?;
    (entry.project == project
        && entry.generation == generation
        && entry.problem.is_none()
        && *change_generation == generation)
        .then(|| change.clone())
}
fn deposit(ctx: &mut Context, job: &Job, e: &mut Entry) -> Reply<()> {
    recovery::connect(&mut ctx.session_control(), job, &e.binding);
    let mut cleanup = ctx.session_control();
    let previous = cleanup.deposit_whole_draft(&e.binding.registration);
    if !matches!(&previous, Ok(Ok(_))) {
        // A lock lost during save leaves the original active draft with the
        // session. Preserve that exact payload before asking the connected
        // sink to accept and acknowledge it. The separate latest-input copy
        // below still runs if this handoff cannot complete.
        e.observations.push(Box::new(previous));
        let preserved = cleanup.preserve_existing(&e.binding.registration);
        let retry = matches!(&preserved, Ok(Ok(_)));
        e.observations.push(Box::new(preserved));
        if retry {
            let retry_result = cleanup.deposit_whole_draft(&e.binding.registration);
            e.observations.push(Box::new(retry_result));
        }
    } else {
        e.observations.push(Box::new(previous));
    }
    drop(cleanup);
    if e.deposit.is_none() {
        let envelope = match &e.submitted {
            Some(s) if s.key.generation == e.generation => s.clone(),
            _ => e.envelope()?,
        };
        e.deposit = Some(
            Deposit::freeze(envelope)
                .map_err(|error| observed(&mut e.observations, error, Code::RecoveryRejected))?,
        );
    }
    let d = e.deposit.as_ref().ok_or(Code::NoDraft)?;
    recovery::capture_assets(ctx, job, d, &mut e.observations)?;
    let store = job
        .recovery
        .connect()
        .map_err(|error| observed(&mut e.observations, error, Code::SinkUnavailable))?;
    let proof = store
        .lock()
        .map_err(|_| Code::Unavailable)?
        .accept(d)
        .map_err(|error| observed(&mut e.observations, error, Code::RecoveryRejected))?;
    if !proof.matches(d.key(), &d.envelope().deposit_id, d.payload_digest()) {
        return Err(Code::NoReceipt.into());
    }
    e.proof = Some(proof);
    Ok(())
}
fn discharge_durable_duplicate(ctx: &mut Context, e: &mut Entry) -> Reply<()> {
    if !e.deposited() {
        return Ok(());
    }
    let durable = e.deposit.as_ref().ok_or(Code::NoReceipt)?;
    let old = ctx
        .session_control()
        .discard_whole_active(&e.binding.registration, |payload| {
            recovery::same_durable_deposit(payload, durable)
        })
        .map_err(|error| observed(&mut e.observations, error, Code::OwnersRemain))?;
    if let Some(old) = old {
        e.observations.push(Box::new(old));
    }
    Ok(())
}
pub(super) fn execute(
    ctx: &mut Context,
    job: &Job,
    project_id: Id,
    request: &Request,
    registry: &mut Registry,
) -> Reply<Completed> {
    match request {
        Request::EditBegin { document } => {
            let id = convert::id(document)?;
            if registry
                .entries
                .values()
                .any(|e| e.document.document().is_ok_and(|d| d.id == id))
            {
                return Err(Code::DuplicateConflict.into());
            }
            if registry.len() >= 16 {
                return Err(Code::Full.into());
            }
            let mut e = begin_entry(ctx, job, project_id, id)?;
            // An existing document has no unsaved input to retain when its
            // first lock acquisition fails. Close that read-only registration
            // and keep the document in read mode so the UI can explain the
            // verified server owner. Retain the entry only if cleanup itself
            // fails and the session still owns a recovery obligation.
            if matches!(
                e.problem.as_deref(),
                Some("SessionRejected" | "RecoveryUnavailable")
            ) {
                let (released, note) = finish_session(ctx, &e.binding);
                if released {
                    let mut error: ErrorDto = if e.problem.as_deref() == Some("RecoveryUnavailable")
                    {
                        Code::SinkUnavailable.into()
                    } else {
                        Code::SessionRejected.into()
                    };
                    if let Some(cause) = e.observations.iter().find_map(|note| {
                        note.downcast_ref::<crate::data::application::worker::SessionRegistration>()
                            .and_then(|registration| registration.original.as_ref().err())
                    }) {
                        error.diagnostic = Some(ErrorDiagnosticDto {
                            stage: format!("{:?}", cause.operation()),
                            category: cause
                                .acquire_error()
                                .map(|lock| format!("{:?}", lock.category()))
                                .unwrap_or_else(|| format!("{:?}", cause.category())),
                            outcome: Some("rejected".into()),
                            io_kind: None,
                            os_code: None,
                            cleanup_outcome: Some("released".into()),
                            cleanup_io_kind: None,
                            cleanup_os_code: None,
                        });
                    }
                    return Err(error);
                }
                e.observations.push(note);
            }
            let mut r = e.response(job.allocated)?;
            r.binding = Some(e.binding.clone());
            registry.entries.insert(job.allocated, e);
            Ok(r)
        }
        Request::EditRestore {
            key,
            deposit_id,
            digest,
        } => {
            if registry.retains(key) || registry.len() >= 16 {
                return Err(Code::DuplicateConflict.into());
            }
            if ctx.project_fingerprint() != Some(key.project_fingerprint.as_str()) {
                return Err(Code::WrongBinding.into());
            }
            let store = job.recovery.connect().map_err(|_| Code::SinkUnavailable)?;
            let store = store.lock().map_err(|_| Code::Unavailable)?;
            let d = store
                .read(key, deposit_id)
                .map_err(|_| Code::RecoveryRejected)?;
            if d.payload_digest() != digest {
                return Err(Code::WrongBinding.into());
            }
            let g = store
                .latest_generation(key)
                .map_err(|_| Code::RecoveryRejected)?
                .checked_add(1)
                .ok_or(Code::Full)?;
            let envelope = d.envelope();
            let (id, template, mut body) = match &envelope.draft {
                Draft::Document {
                    document: Some(id),
                    template,
                    name,
                    english_name,
                    glossary_summary,
                    glossary_excluded,
                    fields,
                    ..
                } => (
                    id,
                    template,
                    EditBody {
                        name: name.clone(),
                        english_name: english_name.clone(),
                        glossary_summary: glossary_summary.clone(),
                        glossary_excluded: glossary_excluded.clone(),
                        fields: fields.clone(),
                        composing: false,
                    },
                ),
                Draft::AdmittedDocument {
                    document,
                    template,
                    edits,
                    ..
                }
                | Draft::AdmittedComposite {
                    document,
                    template,
                    edits,
                    ..
                } => {
                    let mut body = EditBody {
                        name: Intent::Keep,
                        english_name: Intent::Keep,
                        glossary_summary: Intent::Keep,
                        glossary_excluded: Intent::Keep,
                        fields: vec![],
                        composing: false,
                    };
                    for edit in edits {
                        match edit {
                            DocumentEdit::Rename { name } => body.name = Intent::Set(name.clone()),
                            DocumentEdit::EnglishName { value } => {
                                body.english_name = Intent::Set(value.clone())
                            }
                            DocumentEdit::GlossarySummary { value } => {
                                body.glossary_summary = Intent::Set(value.clone())
                            }
                            DocumentEdit::GlossaryExcluded { excluded } => {
                                body.glossary_excluded = Intent::Set(*excluded)
                            }
                            DocumentEdit::Set { field, value } => {
                                body.fields
                                    .push(crate::data::edit_recovery::model::DraftValue {
                                        field: field.clone(),
                                        value: Intent::Set(value.clone()),
                                    })
                            }
                            DocumentEdit::Unset { field } => {
                                body.fields
                                    .push(crate::data::edit_recovery::model::DraftValue {
                                        field: field.clone(),
                                        value: Intent::Unset,
                                    })
                            }
                        }
                    }
                    (document, template, body)
                }
                _ => return Err(Code::WrongBinding.into()),
            };
            let id = convert::id(id)?;
            if registry
                .entries
                .values()
                .any(|e| e.document.document().is_ok_and(|d| d.id == id))
            {
                return Err(Code::DuplicateConflict.into());
            }
            let restoration =
                ctx.read(|ready| store.restore_assets(&d, ready.locked_project().canonical_root()));
            if !matches!(restoration, Ok(Ok(()))) {
                return Ok(Completed::reject(
                    restoration,
                    Code::RecoveryRejected.into(),
                ));
            }
            let mut notes = vec![];
            let (current, tmpl) = load(ctx, id, &mut notes)?;
            if tmpl.template()?.id.to_string() != *template {
                return Err(Code::WrongBinding.into());
            }
            let mut recovered_scan_change = None;
            for (kind, view) in [
                (OriginalKind::Document, &current),
                (OriginalKind::Template, &tmpl),
            ] {
                let old = envelope
                    .originals
                    .iter()
                    .find(|o| o.kind == kind)
                    .ok_or(Code::WrongBinding)?;
                // 자기 committed 결과의 candidate digest는 외부 변경의 rebase 근거와 구분한다.
                let own_commit = kind == OriginalKind::Document
                    && envelope.attempt.as_ref().is_some_and(|a| {
                        a.result == crate::data::edit_recovery::model::SaveState::Committed
                            && a.candidate_digest.as_ref() == source_digest(view).ok().as_ref()
                    });
                if old.source_digest != source_digest(view)? && !own_commit {
                    return Err(Code::WrongBinding.into());
                }
                if own_commit && old.source_digest != source_digest(view)? {
                    let previous = artifact::decode_document(old.snapshot.as_bytes())
                        .map_err(|_| Code::RecoveryRejected)?;
                    if let (View::Document(current), View::Template(template)) = (&**view, &*tmpl) {
                        let previous_id = old
                            .artifact_id
                            .parse()
                            .map_err(|_| Code::RecoveryRejected)?;
                        let previous_byte_length = usize::try_from(old.source_byte_length)
                            .map_err(|_| Code::RecoveryRejected)?;
                        let previous_sha256 =
                            sha256_bytes(&old.source_digest).ok_or(Code::RecoveryRejected)?;
                        let previous_schema =
                            crate::data::schema::SchemaVersion::try_from(old.schema)
                                .map_err(|_| Code::RecoveryRejected)?;
                        let previous_revision =
                            artifact::TemplateRevision::try_from(old.template_revision)
                                .map_err(|_| Code::RecoveryRejected)?;
                        recovered_scan_change = Some(
                            DocumentScanChange::recovered_commit(
                                previous_id,
                                previous_byte_length,
                                previous_sha256,
                                previous_schema,
                                previous_revision,
                                current,
                            )
                            .ok_or(Code::RecoveryRejected)?,
                        );
                        projection::rebase_group_drafts(
                            &mut body.fields,
                            &previous,
                            current.artifact(),
                            template.artifact(),
                        )?;
                    }
                }
            }
            if let Draft::AdmittedComposite { edit, .. } = &envelope.draft {
                let View::Template(t) = &*tmpl else {
                    return Err(Code::WrongBinding.into());
                };
                if !convert::intent(edit)?
                    .is_unchanged(t.artifact(), &timestamp()?)
                    .map_err(|_| Code::CompositeIntentPending)?
                {
                    return Err(Code::CompositeIntentPending.into());
                }
            }
            let mut e = begin_entry(ctx, job, project_id, id)?;
            // 다시 읽는 사이 외부 변경도 거부하며 실패 owner는 정상 해제 경로에 남긴다.
            if source_digest(&e.document)? != source_digest(&current)?
                || source_digest(&e.template)? != source_digest(&tmpl)?
            {
                e.problem = Some("SourceChanged".into());
            }
            e.draft = key.draft_id.clone();
            e.generation = g;
            e.saved = None;
            e.body = body;
            e.attempt = envelope.attempt.clone();
            if e.problem.is_none() {
                e.scan_change = recovered_scan_change.map(|change| (g, change));
            }
            // 원 보관 파일과 두 대상 의도는 해제/저장 성공으로 삭제하지 않는다.
            e.observations.push(Box::new(d));
            let mut r = e.response(job.allocated)?;
            r.binding = Some(e.binding.clone());
            registry.entries.insert(job.allocated, e);
            Ok(r)
        }
        Request::EditDraft {
            owner,
            generation,
            body,
            ..
        }
        | Request::EditDeposit {
            owner,
            generation,
            body,
        } => {
            let e = registry.entries.get_mut(owner).ok_or(Code::UnknownId)?;
            if e.project != project_id {
                return Err(Code::WrongBinding.into());
            }
            e.update(generation, body)?;
            if matches!(request, Request::EditDeposit { .. }) {
                deposit(ctx, job, e)?;
            } else if matches!(request, Request::EditDraft { save: true, .. }) {
                save(ctx, job, e)?;
            }
            e.response(*owner)
        }
        Request::EditRefresh { owner } => {
            let e = registry.entries.get_mut(owner).ok_or(Code::UnknownId)?;
            if e.project != project_id {
                return Err(Code::WrongBinding.into());
            }
            refresh(ctx, job, e)?;
            e.response(*owner)
        }
        Request::EditRetry {
            owner,
            generation: raw,
            body,
        } => {
            let e = registry.entries.get_mut(owner).ok_or(Code::UnknownId)?;
            if e.project != project_id
                || e.expected.is_some()
                || e.problem.as_deref() == Some("Uncertain")
            {
                return Err(Code::WrongBinding.into());
            }
            e.update(raw, body)?;
            let (d, t) = load(ctx, e.document.document()?.id, &mut e.observations)?;
            if source_digest(&d)? != source_digest(&e.document)?
                || source_digest(&t)? != source_digest(&e.template)?
            {
                return Err(Code::WrongBinding.into());
            }
            deposit(ctx, job, e)?;
            discharge_durable_duplicate(ctx, e)?;
            let (released, note) = finish_session(ctx, &e.binding);
            e.observations.push(note);
            if !released {
                return Err(Code::ReleaseRejected.into());
            }
            let target = e.document.target().path().map_err(|_| Code::InvalidInput)?;
            let (mut binding, registration) = begin(ctx, job, vec![target])?;
            binding.id = *owner;
            e.problem = registration
                .original
                .as_ref()
                .err()
                .map(|_| "SessionRejected".into());
            e.observations.push(Box::new(registration));
            e.binding = binding;
            e.generation = e.generation.checked_add(1).ok_or(Code::Full)?;
            e.deposit = None;
            e.proof = None;
            e.pending_scan_change = None;
            e.scan_change = None;
            let mut r = e.response(*owner)?;
            r.binding = Some(e.binding.clone());
            Ok(r)
        }
        Request::EditRelease {
            owner,
            generation: raw,
        } => {
            let e = registry.entries.get_mut(owner).ok_or(Code::UnknownId)?;
            if e.project != project_id || generation(raw)? != e.generation {
                return Err(Code::WrongBinding.into());
            }
            if e.saved != Some(e.generation) && !e.deposited() {
                return Err(Code::NoReceipt.into());
            }
            discharge_durable_duplicate(ctx, e)?;
            let (released, note) = finish_session(ctx, &e.binding);
            e.observations.push(note);
            if !released {
                return Err(Code::ReleaseRejected.into());
            }
            let original = registry.entries.remove(owner);
            let mut r = reply(Response::Released {});
            r.original = Box::new(original);
            r.removed_binding = Some(*owner);
            Ok(r)
        }
        _ => Err(Code::InvalidInput.into()),
    }
}
