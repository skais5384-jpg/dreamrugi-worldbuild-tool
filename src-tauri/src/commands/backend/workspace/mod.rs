//! 앱의 전체 초안 owner. 같은 worker 작업에서 원본·제출 세대·receipt를 함께 판정한다.
use super::*;
use crate::commands::workspace::{
    ContentChunk, DraftAction, DraftProblem, DraftStatus, ReceiptDto, TemplateBody,
};
use crate::data::{
    artifact::{
        template_mutation::whole::{FieldDraftInput, TemplateDraftInput},
        FieldId, FieldKind, OptionId,
    },
    edit_recovery::model::{DraftConfiguration, DraftField, DraftOption, Envelope, Intent, Key},
};
use std::collections::{BTreeMap, BTreeSet};
pub(crate) mod center;
mod operations;
pub(crate) use operations::{control, execute};

#[derive(Default)]
pub(crate) struct Registry {
    pub(super) documents: super::document_workspace::Registry,
    entries: BTreeMap<Id, Entry>,
    selections: BTreeMap<Id, center::Selected>,
    pub(crate) owners: Arc<std::sync::atomic::AtomicUsize>,
}

impl Registry {
    pub(super) fn has_template_owner(&self, project: Id) -> bool {
        self.entries.values().any(|entry| entry.project == project)
    }
    pub(super) fn sync_owners(&self) {
        self.owners.store(
            self.entries.len() + self.documents.len(),
            std::sync::atomic::Ordering::Release,
        );
    }

    pub(crate) fn release_project(&mut self, project: Id) {
        debug_assert!(!self.entries.values().any(|entry| entry.project == project));
        self.documents.release_project(project);
    }

    #[cfg(test)]
    pub(crate) fn has_project_cache(&self, project: Id) -> bool {
        self.documents.has_project_cache(project)
    }
}

struct Entry {
    owner: Id,
    project: Id,
    fingerprint: String,
    draft_id: String,
    base: Option<Arc<View>>,
    seed: Option<artifact::TemplateArtifact>,
    body: TemplateBody,
    generation: u64,
    saved_generation: Option<u64>,
    snapshot: Id,
    content: String,
    phase: &'static str,
    receipt: Option<ReceiptDto>,
    error: Option<ErrorDto>,
    problems: Vec<DraftProblem>,
    outcome: Option<Box<ResultDto>>,
    submitted: Option<Record>,
    deposit: Option<Record>,
    // commit/readback 결과를 transport ack와 독립적으로 유지한다.
    original: Option<Box<dyn Any + Send>>,
    expected_digest: Option<String>,
    restored_from: Option<Key>,
    proof: Option<crate::data::edit_recovery::Proof>,
    release_first: Option<Box<dyn Any + Send>>,
    release_latest: Option<Box<dyn Any + Send>>,
    handoff_receipt: Option<Receipt>,
    handoff_failure: Option<Box<dyn Any + Send>>,
    discarded_input: Option<PendingEdit>,
}
#[derive(Clone)]
struct Record {
    envelope: Envelope,
    attempt: Arc<std::sync::OnceLock<crate::data::edit_recovery::model::Attempt>>,
}

impl Entry {
    fn artifact(&self) -> Reply<&artifact::TemplateArtifact> {
        match &self.base {
            Some(v) => match &**v {
                View::Template(t) => Ok(t.artifact()),
                _ => Err(Code::WrongBinding.into()),
            },
            None => self.seed.as_ref().ok_or_else(|| Code::WrongBinding.into()),
        }
    }
    fn status(&self) -> Reply<DraftStatus> {
        let t = self.artifact()?;
        let source_digest = match &self.base {
            Some(v) => Some(
                v.template()?
                    .token
                    .sha256()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            ),
            None => None,
        };
        let mut identities = BTreeMap::new();
        for field in draft_fields(&self.body.fields) {
            let canonical = if field
                .id
                .strip_prefix("new:")
                .is_some_and(crate::data::edit_recovery::model::valid_id)
            {
                allocated_id(&self.draft_id, "field", &field.id)?
            } else {
                field.id.clone()
            };
            if field.id != canonical {
                identities.insert(field.id.clone(), canonical.clone());
            }
            if let DraftConfiguration::SingleChoice { options }
            | DraftConfiguration::MultiChoice { options } = &field.configuration
            {
                for option in options {
                    if option
                        .id
                        .strip_prefix("new:")
                        .is_some_and(crate::data::edit_recovery::model::valid_id)
                    {
                        identities.insert(
                            option.id.clone(),
                            allocated_id(
                                &self.draft_id,
                                &format!("option/{canonical}"),
                                &option.id,
                            )?,
                        );
                    }
                }
            }
        }
        Ok(DraftStatus {
            owner: self.owner,
            draft_id: self.draft_id.clone(),
            project_fingerprint: self.fingerprint.clone(),
            artifact: t.template_id().to_string(),
            generation: self.generation.to_string(),
            saved_generation: self.saved_generation.map(|g| g.to_string()),
            base_revision: t.revision().get().to_string(),
            source_digest,
            snapshot: self.snapshot,
            phase: self.phase,
            receipt: self.receipt.clone(),
            error: self.error.clone(),
            problems: self.problems.clone(),
            identities,
            outcome: self.outcome.clone(),
        })
    }
    fn rebuild_content(&mut self) -> Reply<()> {
        // 읽기 projection만 포함한다. 큰 원본은 backend에 두고 전송은 고정 snapshot의 조각이다.
        #[derive(serde::Serialize)]
        struct Content<'a> {
            base: TemplateDto,
            body: &'a TemplateBody,
        }
        let content = serde_json::to_string(&Content {
            base: projection::template(self.artifact()?)?,
            body: &self.body,
        })
        .map_err(|_| Code::SerializationFailed)?;
        if content.len() > crate::data::edit_recovery::model::MAX_FILE_BYTES {
            return Err(Code::Full.into());
        }
        self.content = content;
        self.snapshot = Id::new();
        Ok(())
    }
    fn record(&self, job: &Job, submitted: bool) -> Reply<Record> {
        let originals = self
            .base
            .iter()
            .map(|v| {
                recovery::original(v).map_err(|e| {
                    if let Some(b) = &job.binding {
                        b.recovery.record(Some(Arc::new(e)));
                    }
                    ErrorDto::new(Code::RecoveryRejected)
                })
            })
            .collect::<Reply<_>>()?;
        Ok(Record {
            attempt: Arc::new(std::sync::OnceLock::new()),
            envelope: Envelope {
                key: Key {
                    project_fingerprint: self.fingerprint.clone(),
                    draft_id: self.draft_id.clone(),
                    generation: self.generation,
                },
                deposit_id: Id::new().into(),
                app_version: env!("CARGO_PKG_VERSION").into(),
                created_at_utc: timestamp()?,
                originals,
                draft: self.body.recovery(
                    self.base
                        .as_ref()
                        .map(|_| self.artifact().map(|a| a.template_id().to_string()))
                        .transpose()?,
                ),
                attempt: submitted.then(|| crate::data::edit_recovery::model::Attempt {
                    submitted_generation: self.generation,
                    operation_id: job.operation.into(),
                    result: crate::data::edit_recovery::model::SaveState::Unknown,
                    candidate_digest: None,
                    transaction_id: None,
                }),
            },
        })
    }
    fn payload(&self, job: &Job, record: &Record) -> PendingEdit {
        PendingEdit {
            input: job.input.clone(),
            views: self.base.iter().cloned().collect(),
            recovery: recovery::PendingRecovery::whole(
                record.envelope.clone(),
                record.attempt.clone(),
            ),
        }
    }
    fn owns_payload(&self, p: &PendingEdit) -> bool {
        let key = &p.recovery.envelope.key;
        key.project_fingerprint == self.fingerprint
            && key.draft_id == self.draft_id
            && key.generation <= self.generation
    }
    fn accept_body(&mut self, generation: &str, body: &TemplateBody) -> Reply<()> {
        let generation = parse_generation(generation)?;
        if generation < self.generation || (generation == self.generation && body != &self.body) {
            return Err(Code::WrongBinding.into());
        }
        if generation > self.generation {
            self.generation = generation;
            self.body = body.clone();
            self.deposit = None;
            self.receipt = None;
        }
        self.rebuild_content()
    }
}

fn parse_generation(s: &str) -> Reply<u64> {
    let n = s.parse::<u64>().map_err(|_| Code::InvalidInput)?;
    if n == 0 || n.to_string() != s {
        return Err(Code::InvalidInput.into());
    }
    Ok(n)
}

fn body_from_source(t: &artifact::TemplateArtifact) -> Reply<TemplateBody> {
    let dto = projection::template(t)?;
    let mut order = dto.field_order.clone();
    order.extend(
        dto.fields
            .iter()
            .filter(|f| f.lifecycle == "Archived")
            .map(|f| f.id.clone()),
    );
    let mut fields = Vec::new();
    for id in order {
        let f = dto
            .fields
            .iter()
            .find(|f| f.id == id)
            .ok_or(Code::WrongBinding)?;
        let mut option_order = f.option_order.clone();
        option_order.extend(
            f.options
                .iter()
                .filter(|o| o.lifecycle == "Archived")
                .map(|o| o.id.clone()),
        );
        let options = option_order
            .iter()
            .map(|id| {
                let o = f
                    .options
                    .iter()
                    .find(|o| &o.id == id)
                    .ok_or(Code::WrongBinding)?;
                Ok(DraftOption {
                    id: o.id.clone(),
                    label: o.label.clone(),
                    archived: o.lifecycle == "Archived",
                })
            })
            .collect::<Reply<Vec<_>>>()?;
        let configuration = match f.kind.as_str() {
            "Group" => DraftConfiguration::Group {
                card_title_field: Intent::Keep,
                members: body_from_source(&t.member_scope(convert::id(&f.id)?))?.fields,
            },
            "SingleLineText" => DraftConfiguration::SingleLineText {},
            "RichText" => DraftConfiguration::RichText {},
            "Number" => DraftConfiguration::Number {
                minimum: f.minimum.clone(),
                maximum: f.maximum.clone(),
            },
            "Date" => DraftConfiguration::Date {},
            "Time" => DraftConfiguration::Time {},
            "Image" => DraftConfiguration::Image {},
            "File" => DraftConfiguration::File {},
            "Url" => DraftConfiguration::Url {},
            "Duration" => DraftConfiguration::Duration {},
            "SingleChoice" => DraftConfiguration::SingleChoice { options },
            "MultiChoice" => DraftConfiguration::MultiChoice { options },
            "Relation" => DraftConfiguration::Relation {
                multiple: f.multiple.unwrap_or(true),
                allowed_templates: f.allowed_templates.clone(),
                reciprocal_notice: f.reciprocal_notice.unwrap_or(true),
            },
            "DocumentLink" => DraftConfiguration::DocumentLink {},
            _ => return Err(Code::InvalidInput.into()),
        };
        fields.push(DraftField {
            writing_guide: None,
            id: f.id.clone(),
            label: f.label.clone(),
            configuration,
            required: f.required,
            presentation: Intent::Keep,
            default: Intent::Keep,
            archived: f.lifecycle == "Archived",
        });
    }
    Ok(TemplateBody {
        sections: t.sections().iter().map(|s| s.input()).collect(),
        name: dto.name,
        glossary_excluded: dto.glossary_excluded,
        presentation: Intent::Keep,
        fields,
        composing: false,
    })
}

/// 새 항목은 backend가 발급한 논리적 draft ID와 역할에 묶인다. 복원·재시도에도 매핑이 같다.
fn allocated_id(draft: &str, role: &str, raw: &str) -> Reply<String> {
    let local = raw.strip_prefix("new:").ok_or(Code::InvalidInput)?;
    if !crate::data::edit_recovery::model::valid_id(local) {
        return Err(Code::InvalidInput.into());
    }
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("worldbuild-draft-v1/{draft}/{role}/{local}").as_bytes());
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 15) | 0x40;
    bytes[8] = (bytes[8] & 63) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

fn normalize(entry: &Entry) -> Reply<TemplateDraftInput> {
    normalize_parts(entry.artifact()?, &entry.draft_id, &entry.body)
}
fn normalize_parts(
    source: &artifact::TemplateArtifact,
    draft_id: &str,
    body: &TemplateBody,
) -> Reply<TemplateDraftInput> {
    use artifact::template_mutation::{
        FieldValueDraft, NewChoiceOptionDraft, NewFieldConfiguration as C,
    };
    let field_id = |raw: &str| -> Reply<FieldId> {
        if raw.starts_with("new:") {
            return convert::id(&allocated_id(draft_id, "field", raw)?);
        }
        let id = convert::id(raw)?;
        if !source.fields().contains_key(&id) {
            return Err(Code::WrongBinding.into());
        }
        Ok(id)
    };
    let option_id = |field: FieldId, raw: &str| -> Reply<OptionId> {
        if raw.starts_with("new:") {
            return convert::id(&allocated_id(draft_id, &format!("option/{field}"), raw)?);
        }
        let id = convert::id(raw)?;
        if source
            .fields()
            .get(&field)
            .and_then(|f| f.configuration().options())
            .is_none_or(|o| !o.contains_key(&id))
        {
            return Err(Code::WrongBinding.into());
        }
        Ok(id)
    };
    let presentation = |intent: &Intent<String>, previous: Option<&str>| match intent {
        Intent::Keep => previous.map(str::to_owned),
        Intent::Unset => None,
        Intent::Set(v) => Some(v.clone()),
    };
    let mut fields = Vec::new();
    for raw in &body.fields {
        let id = field_id(&raw.id)?;
        let original = source.fields().get(&id);
        let mut archived_options = BTreeSet::new();
        let options = match &raw.configuration {
            DraftConfiguration::SingleChoice { options }
            | DraftConfiguration::MultiChoice { options } => options.as_slice(),
            _ => &[],
        };
        let mut order = Vec::new();
        let mut definitions = Vec::new();
        for o in options {
            let oid = option_id(id, &o.id)?;
            order.push(oid);
            definitions.push(NewChoiceOptionDraft::new(oid, o.label.clone()));
            if o.archived {
                archived_options.insert(oid);
            }
        }
        let (kind, configuration) = match raw.configuration {
            DraftConfiguration::Group { .. } => (FieldKind::Group, C::group()),
            DraftConfiguration::SingleLineText {} => {
                (FieldKind::SingleLineText, C::single_line_text())
            }
            DraftConfiguration::RichText {} => (FieldKind::RichText, C::rich_text()),
            DraftConfiguration::Number {
                ref minimum,
                ref maximum,
            } => (
                FieldKind::Number,
                C::bounded_number(
                    minimum.clone().filter(|v| !v.is_empty()),
                    maximum.clone().filter(|v| !v.is_empty()),
                ),
            ),
            DraftConfiguration::Date {} => (FieldKind::Date, C::date()),
            DraftConfiguration::Time {} => (FieldKind::Time, C::time()),
            DraftConfiguration::Image {} => (FieldKind::Image, C::image()),
            DraftConfiguration::File {} => (FieldKind::File, C::file()),
            DraftConfiguration::Url {} => (FieldKind::Url, C::url()),
            DraftConfiguration::Duration {} => (FieldKind::Duration, C::duration()),
            DraftConfiguration::SingleChoice { .. } => (
                FieldKind::SingleChoice,
                C::single_choice(order, definitions),
            ),
            DraftConfiguration::MultiChoice { .. } => {
                (FieldKind::MultiChoice, C::multi_choice(order, definitions))
            }
            DraftConfiguration::Relation {
                multiple,
                ref allowed_templates,
                reciprocal_notice,
            } => (
                FieldKind::Relation,
                C::relation(
                    multiple,
                    allowed_templates
                        .iter()
                        .map(|value| convert::id(value))
                        .collect::<Reply<_>>()?,
                    reciprocal_notice,
                ),
            ),
            DraftConfiguration::DocumentLink {} => (FieldKind::DocumentLink, C::document_link()),
        };
        let default = match &raw.default {
            Intent::Keep if original.is_some() => None,
            Intent::Keep | Intent::Unset => Some(FieldValueDraft::unset()),
            Intent::Set(value) => Some(match value {
                ValueDto::SingleChoice { option } => {
                    FieldValueDraft::single_choice(option_id(id, option)?)
                }
                ValueDto::MultiChoice { options } => {
                    let mut ids = options
                        .iter()
                        .map(|o| option_id(id, o))
                        .collect::<Reply<Vec<_>>>()?;
                    ids.sort();
                    FieldValueDraft::multi_choice(ids)
                }
                _ => value.default_draft()?,
            }),
        };
        let card_title_field = match &raw.configuration {
            DraftConfiguration::Group {
                members,
                card_title_field: Intent::Set(title),
            } => {
                let member = members
                    .iter()
                    .find(|m| &m.id == title)
                    .ok_or(Code::WrongBinding)?;
                if member.archived
                    || !matches!(member.configuration, DraftConfiguration::RichText {})
                {
                    return Err(Code::WrongBinding.into());
                }
                let mapped: FieldId = if title.starts_with("new:") {
                    convert::id(&allocated_id(draft_id, "field", title)?)?
                } else {
                    convert::id(title)?
                };
                Intent::Set(mapped)
            }
            DraftConfiguration::Group {
                card_title_field: Intent::Unset,
                ..
            } => Intent::Unset,
            _ => Intent::Keep,
        };
        fields.push(FieldDraftInput {
            card_title_field,
            members: if let DraftConfiguration::Group { members, .. } = &raw.configuration {
                if members
                    .iter()
                    .any(|m| matches!(m.configuration, DraftConfiguration::Group { .. }))
                {
                    return Err(Code::InvalidInput.into());
                }
                normalize_parts(
                    &source.member_scope(id),
                    draft_id,
                    &TemplateBody {
                        sections: vec![],
                        name: String::new(),
                        glossary_excluded: source.glossary_excluded(),
                        presentation: Intent::Keep,
                        fields: members.clone(),
                        composing: false,
                    },
                )?
                .fields
            } else {
                vec![]
            },
            writing_guide: match raw.writing_guide.as_ref() {
                Some(Intent::Set(value)) => Some(value.clone()),
                Some(Intent::Unset) => None,
                Some(Intent::Keep) | None => {
                    original.and_then(|f| f.writing_guide()).map(str::to_owned)
                }
            },
            id,
            label: raw.label.clone(),
            kind,
            configuration,
            required: raw.required,
            presentation_token: presentation(
                &raw.presentation,
                original.and_then(|f| f.presentation().token()),
            ),
            default,
            archived: raw.archived,
            archived_options,
        });
    }
    Ok(TemplateDraftInput {
        sections: body
            .sections
            .iter()
            .map(|s| {
                Ok(artifact::SectionInput {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    before_field: s
                        .before_field
                        .as_ref()
                        .map(|id| field_id(id).map(|f| f.to_string()))
                        .transpose()?,
                })
            })
            .collect::<Reply<_>>()?,
        name: body.name.clone(),
        glossary_excluded: body.glossary_excluded,
        presentation_token: presentation(&body.presentation, source.presentation().token()),
        fields,
    })
}

fn draft_fields(fields: &[DraftField]) -> Vec<&DraftField> {
    let mut result = Vec::new();
    for field in fields {
        result.push(field);
        if let DraftConfiguration::Group { members, .. } = &field.configuration {
            result.extend(members);
        }
    }
    result
}
