//! 업데이트도 기존 종료 owner와 동일한 mutex에서 승인한다. UI의 종료 표시는 증거가 아니다.
use super::*;

impl AppState {
    pub(crate) fn hold_project_startup(&self) {
        self.lock().startup_blocked = true;
    }
    pub(crate) fn allow_project_startup(&self) {
        self.lock().startup_blocked = false;
    }
    pub(crate) fn update_check_allowed(&self) -> bool {
        let state = self.lock();
        !state.closed && !state.revoked && !state.update_hold
    }
    pub(crate) fn verify_recovery_update_handoff(
        &self,
    ) -> Result<Vec<crate::data::edit_recovery::Entry>, &'static str> {
        // 복구 내용을 채택/삭제하지 않고 기존 Store의 실제 읽기 검증 결과만 인계한다.
        let store = self.recovery.connect().map_err(|_| "recovery_handoff")?;
        let store = store.lock().map_err(|_| "recovery_handoff")?;
        let listing = store.list().map_err(|_| "recovery_handoff")?;
        if !listing.complete || listing.entries.iter().any(|entry| entry.error.is_some()) {
            return Err("recovery_handoff");
        }
        for entry in &listing.entries {
            store
                .verify_handoff(
                    entry.key.as_ref().ok_or("recovery_handoff")?,
                    entry.deposit_id.as_deref().ok_or("recovery_handoff")?,
                )
                .map_err(|_| "recovery_handoff")?;
        }
        Ok(listing.entries)
    }
    pub(crate) fn prepare_update(&self, candidate: &str) -> Result<(), &'static str> {
        let mut state = self.lock();
        if state.closed
            || state.revoked
            || state.update_hold
            || !state.ui_close.enabled
            || state.ui_close.attempt.is_some()
        {
            return Err("shutdown_blocked");
        }
        state.update_intent = Some(candidate.into());
        state.update_hold = true;
        ui_close::request(&mut state);
        let wake = state.wake.clone();
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
        Ok(())
    }

    pub(crate) fn update_intent(&self) -> Option<String> {
        self.lock().update_intent.clone()
    }
    pub(crate) fn update_work_allowed(&self) -> bool {
        let state = self.lock();
        !state.closed && !state.revoked && !state.update_hold && !state.startup_blocked
    }

    pub(crate) fn ordinary_shutdown(&self) -> bool {
        let state = self.lock();
        state.closed && !state.update_hold
    }
    pub(crate) fn consume_update_approval(&self, candidate: &str) -> bool {
        let mut state = self.lock();
        collect(&mut state);
        native::refresh(&mut state);
        if !state.approved
            || state.exit_sent
            || !state.update_hold
            || state.update_intent.as_deref() != Some(candidate)
        {
            return false;
        }
        // closed는 모든 ordinary 작업을, exit_sent는 control을 포함한 신규 submit을 차단한다.
        state.exit_sent = true;
        state.update_intent = None;
        true
    }

    pub(crate) fn finish_failed_update(&self) {
        let mut state = self.lock();
        state.update_intent = None;
        // 이미 종료한 worker를 가짜 복원하지 않는다. 실패 안내 후 명시 닫기를 기다린다.
        state.update_hold = true;
        state.generation = state.generation.saturating_add(1);
    }

    pub(crate) fn close_after_update_failure(&self) {
        let mut state = self.lock();
        if state.closed && state.update_intent.is_none() {
            state.update_hold = false;
            state.exit_sent = false;
        }
        let wake = state.wake.clone();
        drop(state);
        if let Some(wake) = wake {
            wake();
        }
    }
}
