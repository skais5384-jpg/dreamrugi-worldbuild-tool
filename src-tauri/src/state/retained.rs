//! 보관 입력은 조회나 transport ack로 버리지 않는다. 실제 목적 owner와 같은 lock에서 이동한다.
use super::*;
pub(super) fn list(state: &Inner, caller: Caller) -> Vec<RetainedRef> {
    state
        .retained
        .iter()
        .map(|(_, r)| r)
        .chain(
            state
                .operations
                .values()
                .filter(|o| o.caller == caller)
                .filter_map(|o| o.draft.as_ref().map(|(_, r)| r)),
        )
        .chain(
            state
                .operations
                .values()
                .filter(|o| o.caller == caller)
                .filter_map(|o| o.result.as_ref())
                .filter(|r| r.retain_edit),
        )
        .filter(|r| r.retained_caller == Some(caller.0))
        .filter_map(|r| r.retained)
        .collect()
}

fn matches(result: &Completed, caller: Caller, key: RetainedRef) -> bool {
    result.retained == Some(key) && result.retained_caller == Some(caller.0)
}
fn owner(state: &Inner, caller: Caller, key: RetainedRef) -> Reply<(&Arc<Work>, &Completed)> {
    for op in state.operations.values().filter(|o| o.caller == caller) {
        if let Some((input, result)) = &op.draft {
            if matches(result, caller, key) {
                return Ok((input, result));
            }
        }
        if let Some(result) = &op.result {
            if result.retain_edit && matches(result, caller, key) {
                return Ok((
                    op.edit_input
                        .as_ref()
                        .or(op.input.as_ref())
                        .ok_or(Code::Unavailable)?,
                    result,
                ));
            }
        }
    }
    state
        .retained
        .iter()
        .find(|(_, r)| matches(r, caller, key))
        .map(|(i, r)| (i, r))
        .ok_or_else(|| Code::UnknownId.into())
}
pub(super) fn read(state: &Inner, caller: Caller, key: RetainedRef) -> Reply<Response> {
    let (input, result) = owner(state, caller, key)?;
    Ok(Response::Retained {
        retained: key,
        project: input.project().ok_or(Code::WrongBinding)?,
        artifacts: result
            .sources
            .iter()
            .map(|s| match &**s {
                View::Template(v) => v.artifact().template_id().to_string(),
                View::Document(v) => v.artifact().document_id().to_string(),
            })
            .collect(),
        intent: backend::g6_intent(input),
        result: Box::new(result.dto.clone()?),
        create_retry_safe: matches!(&**input, Work::CreateTemplate { .. })
            && result.binding.is_none()
            && result.g6_clearable,
        g6_clearable: result.g6_clearable
            && result.previous.is_none()
            && !state.operations.values().any(|o| {
                o.pending.is_some()
                    && o.draft
                        .as_ref()
                        .is_some_and(|(_, r)| matches(r, caller, key))
            }),
    })
}
fn eligible(state: &Inner, caller: Caller, key: RetainedRef) -> Reply<()> {
    let (input, result) = owner(state, caller, key)?;
    if !result.g6_clearable
        || result.previous.is_some()
        || backend::g6_intent(input).is_none()
        || state.operations.values().any(|o| {
            o.pending.is_some()
                && o.draft
                    .as_ref()
                    .is_some_and(|(_, r)| matches(r, caller, key))
        })
    {
        return Err(Code::OwnersRemain.into());
    }
    Ok(())
}
pub(super) fn take(
    state: &mut Inner,
    caller: Caller,
    key: RetainedRef,
) -> Reply<(Arc<Work>, Completed)> {
    eligible(state, caller, key)?;
    if let Some(index) = state
        .retained
        .iter()
        .position(|(_, r)| matches(r, caller, key))
    {
        return Ok(state.retained.remove(index));
    }
    for op in state.operations.values_mut().filter(|o| o.caller == caller) {
        if op
            .draft
            .as_ref()
            .is_some_and(|(_, r)| matches(r, caller, key))
        {
            return op.draft.take().ok_or_else(|| Code::UnknownId.into());
        }
        if op
            .result
            .as_ref()
            .is_some_and(|r| r.retain_edit && matches(r, caller, key))
        {
            // 원 operation에는 재조회 가능한 terminal DTO를 남기고 원 결과 allocation만 이동한다.
            let mut old = op.result.take().ok_or(Code::Unavailable)?;
            let mut observation = Completed::new((), old.dto.clone());
            observation.retained = old.retained;
            observation.retained_caller = old.retained_caller;
            old.retain_edit = true;
            op.result = Some(observation);
            return Ok((
                op.edit_input
                    .as_ref()
                    .or(op.input.as_ref())
                    .ok_or(Code::Unavailable)?
                    .clone(),
                old,
            ));
        }
    }
    Err(Code::UnknownId.into())
}
pub(super) fn finish(id: Id, caller: Caller, op: &mut Operation, result: &mut Completed) {
    if let Some(ancestor) = op.draft.take() {
        // 명시적 resume의 이전 원 결과도 새 결과의 transport 인수까지 남긴다.
        // 인수 전 다음 resume를 막아 반복 실패가 무제한 history chain을 만들지 않는다.
        result.previous = Some(Box::new(ancestor));
    }
    if result.retain_edit {
        result.retained = Some(RetainedRef {
            id,
            generation: Id::new(),
        });
        result.retained_caller = Some(caller.0);
    }
}
pub(super) fn handle(
    state: &mut Inner,
    caller: Caller,
    operation: Id,
    input: Arc<Work>,
) -> Reply<bool> {
    let mut project_owner = None;
    let (result, draft) = match &*input {
        Work::RetainedHandoff { retained } => {
            let draft = take(state, caller, *retained)?;
            project_owner = draft.0.project();
            (
                Completed::new(
                    (),
                    Ok(ResultDto::RetainedHandled {
                        retained: *retained,
                        action: "handed_off",
                    }),
                ),
                Some(draft),
            )
        }
        Work::AbandonRetained { retained } => {
            let original = take(state, caller, *retained)?;
            project_owner = original.0.project();
            // 명시적 포기 결과도 transport 인수 전까지 원 owner를 품는다. 범용 P/R 폐기가 아니다.
            (
                Completed::new(
                    original,
                    Ok(ResultDto::RetainedHandled {
                        retained: *retained,
                        action: "abandoned",
                    }),
                ),
                None,
            )
        }
        Work::RetireProject { project } => {
            let p = project_for(state, caller, *project)?;
            let snap = p.control.snapshot();
            let shutdown = shutdown(p);
            // 초기화 전에 runtime을 얻지 못한 worker도 report 인수와 join 뒤에는
            // 실제 owner가 없다. 정상 종료 허가로 둔갑시키지 않고 이 경우만 퇴역시킨다.
            let terminal_initialization_failure = p.joined
                && snap.status == WorkerStatus::InitializationFailed
                && snap.initialization_error.is_some()
                && shutdown.resources_complete
                && !shutdown.report_pending;
            if !p.joined
                || (!shutdown.normal_exit_allowed && !terminal_initialization_failure)
                || (snap.status != WorkerStatus::Stopped && !terminal_initialization_failure)
                || (snap.initialization_error.is_some() && !terminal_initialization_failure)
                || snap.running
                || snap.queued != 0
                || snap.sessions != 0
                || snap.runtime.is_some()
                || !p.views.is_empty()
                || p.force_active != 0
                || !p.force_results.is_empty()
                || p.sessions.values().any(|s| s.draft_input.is_some())
                || state.operations.iter().any(|(id, o)| {
                    *id != operation
                        && (o.project == Some(*project)
                            || o.draft
                                .as_ref()
                                .is_some_and(|(i, _)| i.project() == Some(*project)))
                })
                || state
                    .retained
                    .iter()
                    .any(|(i, _)| i.project() == Some(*project))
            {
                return Err(Code::OwnersRemain.into());
            }
            // 정상 join은 실제 worker session/lock/runtime owner가 모두 끝났다는 하위 증거다.
            // app의 역사적 binding은 최종 관측과 함께 회수하며 release 응답은 app scope에 남긴다.
            let p = state.projects.remove(project).ok_or(Code::UnknownId)?;
            p.control.clear_app_wake();
            (
                Completed::new(
                    shutdown.clone(),
                    Ok(ResultDto::ProjectRetired {
                        project: *project,
                        shutdown,
                    }),
                ),
                None,
            )
        }
        _ => return Ok(false),
    };
    let op = state
        .operations
        .get_mut(&operation)
        .ok_or(Code::UnknownId)?;
    op.input = Some(input);
    op.result = Some(result);
    op.draft = draft;
    op.project = project_owner;
    Ok(true)
}
pub(super) fn resume_input(state: &Inner, caller: Caller, input: &Arc<Work>) -> Reply<Arc<Work>> {
    let Work::ResumeRetained {
        retained,
        project,
        destination,
    } = &**input
    else {
        return Ok(input.clone());
    };
    eligible(state, caller, *retained)?;
    let (original, result) = owner(state, caller, *retained)?;
    if original.project() != Some(*project) {
        return Err(Code::WrongBinding.into());
    }
    let p = project_for(state, caller, *project)?;
    let check_view = |view: Id| -> Reply<()> {
        let view = p.views.get(&view).ok_or(Code::WrongBinding)?;
        if result
            .sources
            .first()
            .is_none_or(|old| old.target() != view.target())
        {
            return Err(Code::WrongBinding.into());
        }
        Ok(())
    };
    // 사용자 의도는 원 typed 값 그대로이며 새 source/session/revision만 명시적으로 받는다.
    let effective = match (&**original, destination) {
        (
            Work::CreateTemplate {
                name, presentation, ..
            },
            G6Destination::Create {},
        ) => Work::CreateTemplate {
            project: *project,
            name: name.clone(),
            presentation: presentation.clone(),
        },
        (Work::DuplicateTemplate { .. }, G6Destination::Duplicate { view }) => {
            check_view(*view)?;
            Work::DuplicateTemplate {
                project: *project,
                view: *view,
            }
        }
        (
            Work::UpdateTemplate { edit, .. },
            G6Destination::Update {
                session,
                view,
                revision,
            },
        ) => {
            check_view(*view)?;
            Work::UpdateTemplate {
                project: *project,
                session: *session,
                view: *view,
                revision: revision.clone(),
                edit: edit.clone(),
            }
        }
        (
            Work::TombstoneTemplate { .. },
            G6Destination::Tombstone {
                session,
                view,
                revision,
            },
        ) => {
            check_view(*view)?;
            Work::TombstoneTemplate {
                project: *project,
                session: *session,
                view: *view,
                revision: revision.clone(),
            }
        }
        _ => return Err(Code::WrongBinding.into()),
    };
    Ok(Arc::new(effective))
}
