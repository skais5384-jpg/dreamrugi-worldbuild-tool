//! native 제거 자격과 제거 완료 뒤 exit 승인을 분리한다. poll은 실패를 재시도하지 않는다.
use super::*;
pub(super) struct Cleanup {
    phase: NativePhase,
    generation: u64,
    attempts: u64,
    first_error: Option<Code>,
    latest_error: Option<Code>,
}
impl Default for Cleanup {
    fn default() -> Self {
        Self {
            phase: NativePhase::Complete,
            generation: 0,
            attempts: 0,
            first_error: None,
            latest_error: None,
        }
    }
}
fn owners_ready(state: &Inner) -> bool {
    state.closed
        && state
            .workspace_owners
            .load(std::sync::atomic::Ordering::Acquire)
            == 0
        && state.operations.values().all(|o| o.input.is_none())
        && state.retained.is_empty()
        && state.projects.values().all(|p| {
            p.joined
                && p.control.shutdown_snapshot().normal_exit_allowed
                && p.reports.is_empty()
                && p.failures.is_empty()
                && p.force_active == 0
                && p.force_results.is_empty()
        })
        && !matches!(
            state.event_error,
            Some(Code::Unavailable | Code::PlatformRegistration)
        )
}
pub(super) fn refresh(state: &mut Inner) {
    let ready = owners_ready(state);
    if ready && state.native.phase == NativePhase::Registered {
        state.native.phase = NativePhase::Pending;
        state.generation = state.generation.saturating_add(1);
    }
    state.approved = ready && state.native.phase == NativePhase::Complete;
}
pub(super) fn failed(state: &mut Inner) {
    state.native.phase = NativePhase::Failed;
    state
        .native
        .first_error
        .get_or_insert(Code::PlatformRemoval);
    state.native.latest_error = Some(Code::PlatformRemoval);
    state.approved = false;
    state.generation = state.generation.saturating_add(1);
}
pub(super) fn retry(state: &mut Inner, generation: &str) -> Reply<()> {
    if state.native.generation.to_string() != generation
        && !(state.native.phase == NativePhase::Running
            && state
                .native
                .generation
                .checked_sub(1)
                .is_some_and(|g| g.to_string() == generation))
    {
        return Err(Code::WrongBinding.into());
    }
    match state.native.phase {
        NativePhase::Failed => state.native.phase = NativePhase::Pending,
        NativePhase::Pending | NativePhase::Running => {}
        _ => return Err(Code::OwnersRemain.into()),
    }
    Ok(())
}
pub(super) fn dto(state: &Inner) -> NativeCleanupDto {
    NativeCleanupDto {
        phase: state.native.phase,
        generation: state.native.generation.to_string(),
        attempts: state.native.attempts.to_string(),
        first_error: state.native.first_error,
        latest_error: state.native.latest_error,
        next_action: (state.native.phase == NativePhase::Failed).then_some("retry_native_cleanup"),
    }
}
impl AppState {
    pub(crate) fn native_registered(&self) {
        let mut state = self.lock();
        state.native.phase = NativePhase::Registered;
        state.approved = false;
    }
    pub(crate) fn begin_native_cleanup(&self) -> bool {
        let mut state = self.lock();
        collect(&mut state);
        refresh(&mut state);
        if owners_ready(&state) && state.native.phase == NativePhase::Pending {
            state.native.phase = NativePhase::Running;
            state.native.generation = state.native.generation.saturating_add(1);
            state.native.attempts = state.native.attempts.saturating_add(1);
            state.generation = state.generation.saturating_add(1);
            true
        } else {
            false
        }
    }
    pub(crate) fn native_removed(&self) {
        let mut state = self.lock();
        if state.native.phase == NativePhase::Complete {
            return;
        }
        state.native.phase = NativePhase::Complete;
        if state.event_error == Some(Code::PlatformRemoval) {
            state.event_error = None;
        }
        state.generation = state.generation.saturating_add(1);
        refresh(&mut state);
    }
}
