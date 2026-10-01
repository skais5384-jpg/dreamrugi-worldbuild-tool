use std::sync::Arc;

use tauri::{Runtime, State, Webview};

use crate::state::AppState;

fn authorized<R: Runtime>(
    webview: &Webview<R>,
    state: &State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state
        .caller(webview.label(), webview.window().label())
        .map(|_| ())
        .map_err(|_| "프로그램 정보를 열 권한이 없습니다.".to_string())
}

#[tauri::command]
pub(crate) fn about_version<R: Runtime>(
    app: tauri::AppHandle<R>,
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    authorized(&webview, &state)?;
    Ok(app.package_info().version.to_string())
}

#[tauri::command]
pub(crate) fn about_channel<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
) -> Result<&'static str, String> {
    authorized(&webview, &state)?;
    let channel = crate::release_contract::channel_label(
        env!("WORLDBUILD_BUILD_CHANNEL"),
        env!("WORLDBUILD_PACKAGE_MODE"),
    );
    Ok(channel)
}

#[tauri::command]
pub(crate) fn about_open_link<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, Arc<AppState>>,
    target: String,
) -> Result<(), String> {
    authorized(&webview, &state)?;
    let url = match target.as_str() {
        "blog" => "https://dreamrugi.tistory.com/",
        "email" => "mailto:skais5384@naver.com",
        "source" => "https://github.com/skais5384-jpg/dreamrugi-worldbuild-tool",
        _ => return Err("연락처 선택이 올바르지 않습니다.".into()),
    };
    open(url).map_err(|_| "기본 앱에서 연락처를 열지 못했습니다.".to_string())
}

#[cfg(windows)]
pub(crate) fn open(url: &str) -> Result<(), ()> {
    use windows::{
        core::PCWSTR,
        Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    };
    let target: Vec<u16> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            PCWSTR(target.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as isize <= 32 {
        Err(())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn open(_: &str) -> Result<(), ()> {
    Err(())
}
