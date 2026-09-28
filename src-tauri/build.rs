fn main() {
    println!("cargo:rerun-if-env-changed=WORLDBUILD_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=WORLDBUILD_PACKAGE_MODE");
    let channel = std::env::var("WORLDBUILD_BUILD_CHANNEL").unwrap_or_else(|_| "dev".to_string());
    let mode = std::env::var("WORLDBUILD_PACKAGE_MODE").unwrap_or_else(|_| "dev".to_string());
    match (channel.as_str(), mode.as_str()) {
        ("dev", "dev") | ("github", "test" | "release") | ("store", "test") => {}
        _ => panic!("invalid build channel/package mode combination"),
    }
    if channel == "store" && std::env::var_os("TAURI_SIGNING_PRIVATE_KEY").is_some() {
        panic!("Store build cannot use GitHub updater signing key");
    }
    println!("cargo:rustc-env=WORLDBUILD_BUILD_CHANNEL={channel}");
    println!("cargo:rustc-env=WORLDBUILD_PACKAGE_MODE={mode}");
    // React/IPC 초기화 전에도 같은 빌드 원본의 안전한 native 안내를 사용할 수 있게 포함한다.
    println!("cargo:rerun-if-changed=../src/strings/ko.json");
    let resource: serde_json::Value = serde_json::from_slice(
        &std::fs::read("../src/strings/ko.json").expect("string resource read failed"),
    )
    .expect("string resource JSON invalid");
    let mut generated = String::new();
    for (name, key) in [
        ("TITLE", "app.message02"),
        ("STARTUP_FAILURE", "startup.failed"),
    ] {
        let value = resource[key]
            .as_str()
            .filter(|s| !s.is_empty() && !s.contains('\0'))
            .expect("native string key/type invalid");
        generated.push_str(&format!("pub const {name}: &str = {value:?};\n"));
    }
    std::fs::write(
        std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR missing"))
            .join("native_strings.rs"),
        generated,
    )
    .expect("native string compile failed");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // lib test 실행 파일도 native subclass API가 요구하는 Common Controls v6를 로드한다.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
    }
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(
                // 같은 기본 dependency를 linker가 모든 실행 target에 한 번만 삽입한다.
                tauri_build::WindowsAttributes::new_without_app_manifest(),
            )
            .app_manifest(tauri_build::AppManifest::new().commands(&[
                "guarded",
                "legacy_handoff_status",
                "legacy_handoff_retry",
                "spell_check",
                "spell_cancel",
                "about_version",
                "about_channel",
                "about_open_link",
                "svn_probe",
                "svn_session",
                "svn_login",
                "svn_logout",
                "svn_inspect",
                "svn_checkout",
                "svn_register_preview",
                "svn_register",
                "svn_status",
                "svn_local_document_status",
                "svn_document_lock_owner",
                "svn_force_document_lock",
                "svn_gui_update",
                "svn_gui_cleanup",
                "svn_commit_candidates",
                "svn_schedule_delete",
                "svn_commit",
                "svn_commit_recheck",
                "svn_cancel",
                "diagnostic_recent_events",
            ])),
    )
    .expect("Tauri app command permission generation failed")
}
