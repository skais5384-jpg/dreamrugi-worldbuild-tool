use crate::{
    commands::dto::Code, data::application::scheduler::TriggerReason, platform, state::AppState,
};
use std::sync::Arc;
use tauri::{Emitter, Manager, RunEvent, Runtime, WindowEvent};

#[derive(Clone, serde::Serialize)]
pub(crate) struct Hint {
    generation: String,
}
/// Wry는 창 thread의 run_on_main_thread를 즉시 실행한다. 기존 Tauri 실행기에서
/// 요청해야 실제 다음 event가 생긴다. 이 작업은 입력/exit를 만들거나 완료를 기다리지 않는다.
pub(crate) fn queue_wake(wake: impl FnOnce() + Send + 'static) {
    tauri::async_runtime::spawn(async move { wake() });
}
pub(crate) fn window_event(state: &AppState, label: &str, event: &WindowEvent) {
    if label != "main" {
        return;
    }
    match event {
        WindowEvent::Focused(true) => state.trigger(TriggerReason::Foreground),
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            state.request_shutdown();
        }
        _ => {}
    }
}
pub(crate) fn handle<R: Runtime>(
    app: &tauri::AppHandle<R>,
    event: &RunEvent,
    last_generation: &mut String,
) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    match event {
        #[cfg(windows)]
        RunEvent::Ready => {
            let installed = app.get_webview_window("main").and_then(|main| {
                crate::youtube::install(&main, Arc::clone(&state));
                main.hwnd()
                    .ok()
                    .and_then(|hwnd| platform::install(hwnd, Arc::clone(&state)).ok())
            });
            if installed.is_none() {
                state.event_failure(Code::PlatformRegistration);
                state.request_shutdown();
                state.clear_wake();
            }
        }
        RunEvent::WindowEvent { label, event, .. } => window_event(&state, label, event),
        RunEvent::ExitRequested { api, .. } => {
            if !state.exit_approved() {
                api.prevent_exit();
                state.request_shutdown();
            }
        }
        RunEvent::MainEventsCleared => {
            // collect may complete the last worker join and make native cleanup
            // eligible during this very pass. Run cleanup after that observation;
            // a stopped app might not produce another window event to wake us.
            state.poll();
            cleanup_pass(&state);
            if state.poll() {
                state.clear_wake();
                // AppHandle::exit에는 request 실패 시 강제 종료 fallback이 있어 이 위치에서만 호출한다.
                if !app.state::<crate::svn::Manager>().has_active() && state.take_exit_approval() {
                    app.exit(0);
                }
            } else if state.join_pending() {
                // stop 뒤 thread 반환과 is_finished 관측 사이의 작은 틈을 event loop에서 재확인한다.
                let app = app.clone();
                let state = Arc::clone(&state);
                queue_wake(move || {
                    if app.run_on_main_thread(|| {}).is_err() {
                        state.event_failure(Code::Unavailable);
                    }
                });
            }
            let generation = state.generation();
            if generation != *last_generation {
                *last_generation = generation.clone();
                if let Some(reason) = state.shutdown_wait_reason() {
                    crate::diagnostic_log::record("app", "shutdown", "wait", reason, None, None);
                }
                if app
                    .emit_to("main", "guarded-state", Hint { generation })
                    .is_err()
                {
                    state.event_failure(Code::EventDelivery);
                }
            }
        }
        // 종료 요청이 승인된 시점과 실제 event loop 종료를 구별한다. 창 닫기 취소나
        // 정리 실패 중에는 marker를 유지하고, process 종료가 확정된 이 event에서만
        // 정상 종료로 기록한다.
        RunEvent::Exit => crate::diagnostic_log::mark_clean(),
        _ => {}
    }
}
/// 호출은 창 소유 thread에서만 한다. app mutex를 풀고 실제 native 제거를 수행한다.
pub(crate) fn cleanup_pass(state: &AppState) {
    if state.begin_native_cleanup() {
        match platform::remove() {
            Ok(()) => state.native_removed(),
            Err(_) => state.event_failure(Code::PlatformRemoval),
        }
    }
}
