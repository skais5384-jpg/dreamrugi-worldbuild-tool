//! 미제출 폼 확인은 shutdown보다 먼저 한다. 확인은 최종 종료 승인이 아니다.
use super::*;

#[derive(Default)]
pub(super) struct Guard {
    pub(super) enabled: bool,
    pub(super) attempt: Option<Id>,
}

pub(super) fn status(state: &Inner) -> Response {
    Response::UiClose {
        enabled: state.ui_close.enabled,
        attempt: state.ui_close.attempt,
        closing: state.closed,
    }
}

pub(super) fn register(state: &mut Inner) -> Reply<Response> {
    if state.closed {
        return Err(Code::Closed.into());
    }
    state.ui_close.enabled = true;
    Ok(status(state))
}

pub(super) fn request(state: &mut Inner) {
    if state.closed || !state.ui_close.enabled {
        request_shutdown(state);
    } else if state.ui_close.attempt.is_none() {
        // 반복 close는 같은 시도다. event 유실/timeout으로 입력 포기를 추측하지 않는다.
        state.ui_close.attempt = Some(Id::new());
        state.generation = state.generation.saturating_add(1);
    }
}

pub(super) fn decide(state: &mut Inner, attempt: Id, proceed: bool) -> Reply<Response> {
    if !state.ui_close.enabled || state.ui_close.attempt != Some(attempt) || state.closed {
        return Err(Code::WrongBinding.into());
    }
    state.ui_close.attempt = None;
    if proceed {
        // 기존 worker/retained/native owner의 정상 종료 조건은 그대로 적용한다.
        request_shutdown(state);
    } else {
        state.update_intent = None;
        state.update_hold = false;
    }
    Ok(status(state))
}
