//! YouTube embed 문서 요청에만 데스크톱 앱 식별자를 붙인다.
//! 일반 네트워크 요청이나 플레이어 하위 리소스의 헤더는 바꾸지 않는다.

#[cfg(windows)]
pub(crate) fn install<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    state: std::sync::Arc<crate::state::AppState>,
) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        WebResourceRequestedEventHandler,
    };
    use windows::core::HSTRING;

    const FILTER: &str = "https://www.youtube-nocookie.com/embed/*";
    const REFERER: &str = "https://com.dreamrugi.worldbuildtool/";

    // `with_webview`는 setup thread에서 즉시 WebView2를 주는 API가 아니라 다음 Wry event에
    // 등록 작업을 예약한다. 여기서 동기 결과를 기다리면 setup이 먼저 실패해 앱이 열리지 않는다.
    let callback_state = std::sync::Arc::downgrade(&state);
    let scheduled = window.with_webview(move |platform| {
        let installed = (|| unsafe {
            let webview = platform
                .controller()
                .CoreWebView2()
                .map_err(|error| ("core_webview", error))?;
            webview
                .AddWebResourceRequestedFilter(
                    &HSTRING::from(FILTER),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
                )
                .map_err(|error| ("request_filter", error))?;
            let handler = WebResourceRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    let request = args.Request()?;
                    request
                        .Headers()?
                        .SetHeader(&HSTRING::from("Referer"), &HSTRING::from(REFERER))?;
                }
                Ok(())
            }));
            let mut token = 0;
            webview
                .add_WebResourceRequested(&handler, &mut token)
                .map_err(|error| ("handler_registration", error))?;
            Ok::<(), (&'static str, windows::core::Error)>(())
        })();
        let changed = match installed {
            Ok(()) => crate::support_diagnostics::clear("youtube_handler"),
            Err((stage, error)) => {
                // 전체 COM 문자열 대신 단계와 HRESULT만 남겨 경로·원격 주소 노출을 막는다.
                let changed = crate::support_diagnostics::record(
                    "youtube_handler",
                    stage,
                    "webview2_com",
                    Some(format!("0x{:08X}", error.code().0 as u32)),
                );
                eprintln!("[youtube_handler_install_failed] {stage}");
                changed
            }
        };
        if changed {
            if let Some(state) = callback_state.upgrade() {
                state.support_diagnostics_changed();
            }
        }
    });
    if scheduled.is_err() {
        let changed = crate::support_diagnostics::record(
            "youtube_handler",
            "callback_schedule",
            "webview_dispatch",
            None,
        );
        if changed {
            state.support_diagnostics_changed();
        }
        eprintln!("[youtube_handler_install_failed] callback_schedule");
    }
}
