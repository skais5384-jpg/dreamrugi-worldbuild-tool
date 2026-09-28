//! 실행 중 Work handle을 영속 권한으로 저장하지 않고 실제 입력/원본만 복구에 전달한다.
use super::*;
use crate::data::{
    application::recovery_handoff::RecoveryBackend,
    collaboration_lock::LockSessionId,
    edit_recovery::{
        self,
        error::{Category, RecoveryError, Stage},
        model::*,
    },
    edit_session::{
        DurableRecoverySink, RecoveryEnvelope, RecoverySinkFailure, RecoverySinkFailureCategory,
    },
    project_relative_path::ProjectRelativePath,
    repository::SourceToken,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};

#[derive(Default)]
pub(crate) struct Observation {
    pub(crate) connected: AtomicBool,
    error: Mutex<Errors>,
}
#[derive(Default)]
struct Errors {
    first: Option<Arc<RecoveryError>>,
    latest: Option<Arc<RecoveryError>>,
}
impl Observation {
    pub(super) fn record(&self, error: Option<Arc<RecoveryError>>) {
        // poison 상태도 최초/최신 원인을 보관한다. dto는 poison을 Unavailable로 알린다.
        let mut slot = self
            .error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if slot.first.is_none() {
            slot.first = error.clone();
        }
        slot.latest = error;
    }

    pub(crate) fn dto(&self) -> Option<edit_recovery::error::ErrorDto> {
        match self.error.lock() {
            Ok(error) => error.latest.as_ref().map(|e| e.dto()),
            Err(_) => Some(RecoveryError::new(Category::Unavailable, Stage::Read).dto()),
        }
    }
}
pub(crate) struct PendingRecovery {
    pub(super) envelope: Envelope,
    pub(super) attempt: Arc<OnceLock<Attempt>>,
    frozen: Option<Deposit>,
}
impl PendingRecovery {
    pub(super) fn whole(envelope: Envelope, attempt: Arc<OnceLock<Attempt>>) -> Self {
        Self {
            envelope,
            attempt,
            frozen: None,
        }
    }
}

/// The direct Store receipt can discharge a worker draft only when it covers
/// the exact payload that the worker still owns, including the final save
/// attempt. A matching key alone is not sufficient evidence.
pub(super) fn same_durable_deposit(payload: &PendingEdit, durable: &Deposit) -> bool {
    let candidate = if let Some(frozen) = payload.recovery.frozen.as_ref() {
        Some(frozen.payload_digest().to_owned())
    } else {
        let mut envelope = payload.recovery.envelope.clone();
        if let Some(attempt) = payload.recovery.attempt.get() {
            envelope.attempt = Some(attempt.clone());
        }
        Deposit::freeze(envelope)
            .ok()
            .map(|frozen| frozen.payload_digest().to_owned())
    };
    payload.recovery.envelope.key == *durable.key()
        && payload.recovery.envelope.deposit_id == durable.envelope().deposit_id
        && candidate.as_deref() == Some(durable.payload_digest())
}
impl PendingEdit {
    pub(super) fn new(job: &Job, project: &str) -> Reply<Self> {
        let (document, template, draft) = match &*job.input {
            Work::SaveDocument {
                document,
                template,
                revision,
                edits,
                ..
            } => {
                let document_id = job.view(*document)?.document()?.id.to_string();
                let template_id = job.view(*template)?.template()?.id.to_string();
                (
                    *document,
                    *template,
                    Draft::AdmittedDocument {
                        document: document_id,
                        template: template_id,
                        revision: revision.clone(),
                        edits: edits.clone(),
                    },
                )
            }
            Work::SaveComposite {
                document,
                template,
                revision,
                edit,
                edits,
                ..
            } => {
                let document_id = job.view(*document)?.document()?.id.to_string();
                let template_id = job.view(*template)?.template()?.id.to_string();
                (
                    *document,
                    *template,
                    Draft::AdmittedComposite {
                        document: document_id,
                        template: template_id,
                        revision: revision.clone(),
                        edit: edit.clone(),
                        edits: edits.clone(),
                    },
                )
            }
            _ => return Err(Code::InvalidInput.into()),
        };
        let originals = [template, document]
            .iter()
            .map(|id| {
                original(job.view(*id)?).map_err(|error| {
                    if let Some(binding) = &job.binding {
                        binding.recovery.record(Some(Arc::new(error)));
                    }
                    Code::RecoveryRejected.into()
                })
            })
            .collect::<Reply<Vec<_>>>()?;
        let envelope = Envelope {
            key: Key {
                project_fingerprint: project.to_owned(),
                draft_id: uuid::Uuid::new_v4().to_string(),
                generation: 1,
            },
            deposit_id: uuid::Uuid::new_v4().to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            created_at_utc: timestamp()?,
            originals,
            draft,
            attempt: Some(Attempt {
                submitted_generation: 1,
                operation_id: job.operation.into(),
                result: SaveState::Unknown,
                candidate_digest: None,
                transaction_id: None,
            }),
        };
        Ok(Self {
            input: job.input.clone(),
            views: job.views.iter().map(|(_, v)| v.clone()).collect(),
            recovery: PendingRecovery {
                envelope,
                attempt: Arc::new(OnceLock::new()),
                frozen: None,
            },
        })
    }
}
pub(super) fn original(view: &View) -> Result<Original, RecoveryError> {
    let (kind, token, bytes): (_, &SourceToken, _) = match view {
        View::Template(v) => (
            OriginalKind::Template,
            v.source(),
            artifact::encode_template(v.artifact()),
        ),
        View::Document(v) => (
            OriginalKind::Document,
            v.source(),
            artifact::encode_document(v.artifact()),
        ),
    };
    let bytes =
        bytes.map_err(|e| RecoveryError::caused(Category::InvalidEnvelope, Stage::Validate, e))?;
    let snapshot_digest = digest(&bytes);
    let snapshot = String::from_utf8(bytes)
        .map_err(|e| RecoveryError::caused(Category::InvalidEnvelope, Stage::Validate, e))?;
    let id = match token.id() {
        ArtifactSourceId::Template(id) => id.to_string(),
        ArtifactSourceId::Document(id) => id.to_string(),
        ArtifactSourceId::DocumentLayout => "document-layout".into(),
    };
    let revision = match token.revision() {
        crate::data::repository::SourceRevision::Layout(r) => r,
        crate::data::repository::SourceRevision::Template(r)
        | crate::data::repository::SourceRevision::DocumentTemplate(r) => r.get(),
    };
    Ok(Original {
        kind,
        artifact_id: id,
        schema: token.schema().get(),
        template_revision: revision,
        source_byte_length: token.byte_length() as u64,
        source_digest: token.sha256().iter().map(|b| format!("{b:02x}")).collect(),
        snapshot,
        snapshot_digest,
    })
}
pub(super) fn attempt(operation: Id, diagnostic: Option<ApplicationDiagnostic>) -> Attempt {
    attempt_ref(operation, diagnostic.as_ref())
}
pub(super) fn attempt_ref(operation: Id, diagnostic: Option<&ApplicationDiagnostic>) -> Attempt {
    let state = diagnostic.as_ref().map(|d| d.disk);
    let transaction_id = diagnostic.as_ref().and_then(|d| {
        d.artifact_write
            .as_ref()
            .and_then(|w| w.transaction_id.as_ref().map(ToString::to_string))
            .or_else(|| {
                d.artifact_commit.as_ref().and_then(|c| {
                    c.failures
                        .iter()
                        .find_map(|f| f.transaction_id.as_ref().map(ToString::to_string))
                })
            })
    });
    Attempt {
        submitted_generation: 1,
        operation_id: operation.into(),
        result: match state {
            None => SaveState::Unknown,
            Some(DiskState::NotAttempted) => SaveState::NotAttempted,
            Some(DiskState::NoWrite) => SaveState::NoWrite,
            Some(DiskState::NotApplied) => SaveState::NotApplied,
            Some(DiskState::RolledBack) => SaveState::RolledBack,
            Some(DiskState::Committed) => SaveState::Committed,
            Some(DiskState::Uncertain) => SaveState::Uncertain,
        },
        candidate_digest: None,
        transaction_id,
    }
}

pub(super) fn connect(
    ctx: &mut CleanupContext<'_, PendingEdit, Receipt>,
    job: &Job,
    binding: &Binding,
) {
    let _ = connect_checked(ctx, job, binding);
}
pub(super) fn connect_checked(
    ctx: &mut CleanupContext<'_, PendingEdit, Receipt>,
    job: &Job,
    binding: &Binding,
) -> Result<(), Arc<RecoveryError>> {
    if binding.recovery.connected.load(Ordering::Acquire) {
        return Ok(());
    }
    let result = (|| {
        let (snapshot, _) = ctx
            .observe_session(&binding.registration)
            .map_err(|_| Arc::new(RecoveryError::new(Category::Unavailable, Stage::Initialize)))?;
        let project = snapshot
            .project_fingerprint()
            .ok_or_else(|| {
                Arc::new(RecoveryError::new(
                    Category::InvalidEnvelope,
                    Stage::Initialize,
                ))
            })?
            .to_owned();
        let session = snapshot
            .session_id()
            .ok_or_else(|| {
                Arc::new(RecoveryError::new(
                    Category::InvalidEnvelope,
                    Stage::Initialize,
                ))
            })?
            .clone();
        let store = job.recovery.connect()?;
        let sink = Sink {
            store,
            project,
            session,
            targets: snapshot.targets().to_vec(),
            observation: binding.recovery.clone(),
        };
        ctx.connect_backend(&binding.registration, RecoveryBackend::connected(sink))
            .map_err(|_| Arc::new(RecoveryError::new(Category::Conflict, Stage::Initialize)))?;
        binding.recovery.connected.store(true, Ordering::Release);
        Ok(())
    })();
    binding.recovery.record(result.as_ref().err().cloned());
    result
}
struct Sink {
    store: Arc<Mutex<edit_recovery::Store>>,
    project: String,
    session: LockSessionId,
    targets: Vec<ProjectRelativePath>,
    observation: Arc<Observation>,
}
impl DurableRecoverySink<PendingEdit> for Sink {
    type Receipt = Receipt;
    fn accept_durably(
        &mut self,
        envelope: &RecoveryEnvelope,
        mut payload: PendingEdit,
    ) -> Result<Receipt, RecoverySinkFailure<PendingEdit>> {
        let accepted = (|| {
            if envelope.project_fingerprint() != self.project
                || envelope.session_id() != &self.session
                || envelope.targets() != self.targets
                || payload.recovery.envelope.key.project_fingerprint != self.project
            {
                return Err(RecoveryError::new(
                    Category::InvalidEnvelope,
                    Stage::Validate,
                ));
            }
            if payload.recovery.frozen.is_none() {
                let mut envelope = payload.recovery.envelope.clone();
                // 저장 시도의 최종 관측이 없으면 성공/미적용 어느 쪽으로도 추측하지 않는다.
                if let Some(attempt) = payload.recovery.attempt.get() {
                    envelope.attempt = Some(attempt.clone());
                }
                payload.recovery.frozen = Some(Deposit::freeze(envelope)?);
            }
            let deposit =
                payload.recovery.frozen.as_ref().ok_or_else(|| {
                    RecoveryError::new(Category::InvalidEnvelope, Stage::Validate)
                })?;
            self.store
                .lock()
                .map_err(|_| RecoveryError::new(Category::Unavailable, Stage::Lock))?
                .accept(deposit)
        })();
        match accepted {
            Ok(proof) => {
                self.observation.record(None);
                Ok(Receipt::from_store(proof))
            }
            Err(error) => {
                let category = match error.category {
                    Category::DurabilityUncertain => {
                        RecoverySinkFailureCategory::DurabilityUncertain
                    }
                    Category::Initialization
                    | Category::Permission
                    | Category::Capacity
                    | Category::Busy
                    | Category::Unavailable
                    | Category::Io => RecoverySinkFailureCategory::Unavailable,
                    _ => RecoverySinkFailureCategory::Rejected,
                };
                self.observation.record(Some(Arc::new(error)));
                Err(RecoverySinkFailure::new(payload, category))
            }
        }
    }
}
#[cfg(test)]
mod tests;

/// 저장 전에 별도 복구 사본을 인수한다. 실패 원인은 owner의 관측 자료에 보존한다.
pub(super) fn capture_assets(
    ctx: &mut Context,
    job: &Job,
    deposit: &Deposit,
    observations: &mut Vec<Box<dyn Any + Send>>,
) -> Reply<()> {
    // 첨부가 없는 기존 경로에는 복구 저장소 가용성이라는 새 선행 조건을 붙이지 않는다.
    if !deposit
        .has_assets()
        .map_err(|e| document_workspace::observed(observations, e, Code::RecoveryRejected))?
    {
        return Ok(());
    }
    let store = job
        .recovery
        .connect()
        .map_err(|e| document_workspace::observed(observations, e, Code::SinkUnavailable))?;
    ctx.read(|ready| {
        store
            .lock()
            .map_err(|_| Code::Unavailable)?
            .capture_assets(deposit, ready.locked_project().canonical_root())
            .map_err(|e| document_workspace::observed(observations, e, Code::RecoveryRejected))
    })
    .map_err(|e| document_workspace::observed(observations, e, Code::RuntimeRejected))?
}
