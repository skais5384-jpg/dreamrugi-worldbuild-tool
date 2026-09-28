pub(crate) mod asset_picker;
pub(crate) mod backend;
mod convert;
pub(crate) mod document_workspace;
pub(crate) mod dto;
mod projection;
pub(crate) mod workspace;
#[cfg(test)]
pub(crate) use convert::edits as test_edits;

use crate::state::AppState;
use dto::{Code, Reply, Response};
use std::sync::Arc;
use tauri::{
    ipc::{InvokeBody, Request},
    Runtime, State, Webview,
};

/// Request로 원 body를 받아 기본 serde 오류가 잘못된 tag/민감 입력을 반사하지 않게 한다.
#[tauri::command]
pub(crate) fn guarded<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    request: Request<'_>,
) -> Reply<Response> {
    let caller = state.caller(webview.label(), webview.window().label())?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err(Code::InvalidInput.into());
    };
    state.dispatch(caller, dto::decode(bytes)?)
}

#[tauri::command]
pub(crate) fn legacy_handoff_status<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::data::edit_recovery::HandoffStatus, &'static str> {
    if webview.label() != "main" || webview.window().label() != "main" {
        return Err("invalid_caller");
    }
    Ok(state.recovery_handoff_status())
}

#[tauri::command]
pub(crate) fn legacy_handoff_retry<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<crate::data::edit_recovery::HandoffStatus, &'static str> {
    if webview.label() != "main" || webview.window().label() != "main" {
        return Err("invalid_caller");
    }
    Ok(state.run_recovery_handoff("retry"))
}
pub(crate) fn register<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        guarded,
        legacy_handoff_status,
        legacy_handoff_retry,
        crate::spellcheck::spell_check,
        crate::spellcheck::spell_cancel,
        crate::about::about_version,
        crate::about::about_channel,
        crate::about::about_open_link,
        crate::svn::svn_probe,
        crate::svn::svn_session,
        crate::svn::svn_login,
        crate::svn::svn_logout,
        crate::svn::svn_inspect,
        crate::svn::svn_checkout,
        crate::svn::svn_register_preview,
        crate::svn::svn_register,
        crate::svn::svn_status,
        crate::svn::svn_local_document_status,
        crate::svn::svn_document_lock_owner,
        crate::svn::svn_force_document_lock,
        crate::svn::svn_gui_update,
        crate::svn::svn_gui_cleanup,
        crate::svn::svn_commit_candidates,
        crate::svn::svn_schedule_delete,
        crate::svn::svn_commit,
        crate::svn::svn_commit_recheck,
        crate::svn::svn_cancel,
        crate::diagnostic_log::diagnostic_recent_events
    ])
}
