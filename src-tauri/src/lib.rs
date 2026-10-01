mod about;
mod app_paths;
mod commands;
pub mod data;
mod diagnostic_log;
mod events;
mod pdf_export;
mod platform;
mod release_contract;
mod spellcheck;
mod state;
mod support_diagnostics;
mod svn;
mod svn_guard;
mod svn_shared;
mod updater;
#[cfg(windows)]
mod youtube;
mod native_strings {
    include!(concat!(env!("OUT_DIR"), "/native_strings.rs"));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tauri::Manager;
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            commands::asset_picker::install(app.handle().clone());
            if let Some(main) = app.get_webview_window("main") {
                main.set_title(native_strings::TITLE)?;
                #[cfg(debug_assertions)]
                if std::env::var_os("WORLDBUILD_PDF_PROBE_DIR").is_some() {
                    let _ = main.set_position(tauri::Position::Physical(
                        tauri::PhysicalPosition::new(-32000, -32000),
                    ));
                    let _ = main.set_skip_taskbar(true);
                    let _ = main.show();
                }
            }
            let app_data = app_paths::local_data(app.handle())?;
            svn_guard::configure(app_data.join("svn"));
            spellcheck::cleanup_abandoned(app.handle());
            diagnostic_log::initialize(app_data.join("diagnostics"));
            pdf_export::install(app.handle().clone());
            std::panic::set_hook(Box::new(|_| {
                diagnostic_log::record("app", "panic", "panic_observed", "abnormal", None, None);
                eprintln!("[panic_observed] 안전 진단에 제한된 panic 사건을 기록했습니다.");
            }));
            let svn_manager = svn::Manager::new_with_version(
                app_data.join("svn"),
                app.package_info().version.clone(),
            );
            let state = Arc::new(
                state::AppState::with_recovery_root(
                    app_paths::project_lock_root()?,
                    app_data.join("settings"),
                    app_paths::recovery_root(app.handle())?,
                    app_paths::legacy_recovery_root(app.handle())?,
                )
                .with_svn_manager(svn_manager.clone()),
            );
            // This is deliberately synchronous: an immediate normal exit after
            // opening the home screen cannot abandon a pending legacy handoff.
            state.run_recovery_handoff("startup");
            state.hold_project_startup();
            app.manage(Arc::new(updater::Manager::new(
                app_data.join("settings"),
                app.package_info().version.to_string(),
            )));
            let idle_app = app.handle().clone();
            let idle_state = Arc::clone(&state);
            svn_manager.set_idle_wake(Arc::new(move || {
                if !idle_state.exit_approved() {
                    return;
                }
                let app = idle_app.clone();
                let state = Arc::clone(&idle_state);
                events::queue_wake(move || {
                    if app.run_on_main_thread(|| {}).is_err() {
                        state.event_failure(commands::dto::Code::Unavailable);
                    }
                });
            }));
            let wake_app = app.handle().clone();
            let failure = Arc::new(AtomicBool::new(false));
            let wake_failure = failure.clone();
            state.set_wake(Arc::new(move || {
                let app = wake_app.clone();
                let failure = wake_failure.clone();
                events::queue_wake(move || {
                    if app.run_on_main_thread(|| {}).is_err() {
                        failure.store(true, Ordering::Release);
                    }
                });
            }));
            app.manage(state);
            app.manage(svn_manager);
            app.manage(failure);
            #[cfg(debug_assertions)]
            pdf_export::maybe_probe();
            #[cfg(debug_assertions)]
            spellcheck::maybe_probe(app.handle().clone());
            Ok(())
        });
    let builder = commands::register(builder);
    match builder.build(tauri::generate_context!()) {
        Ok(app) => {
            let mut generation = String::new();
            app.run(move |app, event| {
                if app.state::<svn::Manager>().has_active() {
                    match &event {
                        tauri::RunEvent::WindowEvent {
                            label,
                            event: tauri::WindowEvent::CloseRequested { api, .. },
                            ..
                        } if label == "main" => {
                            api.prevent_close();
                            app.state::<Arc<state::AppState>>().request_shutdown();
                            return;
                        }
                        tauri::RunEvent::ExitRequested { api, .. } => {
                            api.prevent_exit();
                            let state = app.state::<Arc<state::AppState>>();
                            state.defer_exit();
                            state.request_shutdown();
                            return;
                        }
                        _ => {}
                    }
                }
                if matches!(event, tauri::RunEvent::Exit) {
                    app.state::<Arc<updater::Manager>>().cancel_all();
                    spellcheck::cancel_all();
                    app.state::<svn::Manager>().cancel_active();
                }
                if app.state::<Arc<AtomicBool>>().load(Ordering::Acquire) {
                    app.state::<Arc<state::AppState>>()
                        .event_failure(commands::dto::Code::Unavailable);
                }
                events::handle(app, &event, &mut generation);
            });
        }
        Err(_original_error) => {
            // 외부 라이브러리 오류의 전체 경로를 기본 로그에 복사하지 않는다.
            #[cfg(windows)]
            platform::show_startup_failure();
            eprintln!("[startup_failed] {}", native_strings::STARTUP_FAILURE);
            std::process::exit(1);
        }
    }
}
