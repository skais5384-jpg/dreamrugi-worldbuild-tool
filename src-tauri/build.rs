#[allow(dead_code)]
#[path = "src/release_contract.rs"]
mod release_contract;

fn main() {
    println!("cargo:rerun-if-env-changed=WORLDBUILD_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=WORLDBUILD_PACKAGE_MODE");
    let channel = std::env::var("WORLDBUILD_BUILD_CHANNEL").unwrap_or_else(|_| "dev".to_string());
    let mode = std::env::var("WORLDBUILD_PACKAGE_MODE").unwrap_or_else(|_| "dev".to_string());
    match (channel.as_str(), mode.as_str()) {
        ("dev", "dev") | ("github", "test" | "release") | ("store", "test" | "release") => {}
        _ => panic!("invalid build channel/package mode combination"),
    }
    for key in [
        "TAURI_SIGNING_PRIVATE_KEY",
        "TAURI_SIGNING_PRIVATE_KEY_PATH",
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
        "WORLDBUILD_STORE_IDENTITY",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    if channel == "store"
        && [
            "TAURI_SIGNING_PRIVATE_KEY",
            "TAURI_SIGNING_PRIVATE_KEY_PATH",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        panic!("Store build cannot receive GitHub updater signing secrets");
    }
    if channel == "store" && mode == "release" {
        let raw = std::env::var("WORLDBUILD_STORE_IDENTITY")
            .expect("verified Partner Center identity required");
        release_contract::validate_store_release(
            &raw,
            &std::env::var("CARGO_PKG_VERSION").unwrap(),
        )
        .expect("Store identity/history rejected");
        let config: serde_json::Value = serde_json::from_str(
            &std::env::var("TAURI_CONFIG").expect("explicit Store release config required"),
        )
        .expect("Store config JSON");
        if config["identifier"] != "com.dreamrugi.worldbuildtool"
            || config["bundle"]["createUpdaterArtifacts"] != false
        {
            panic!("Store release requires official identity and disabled updater artifacts");
        }
    }
    println!("cargo:rustc-env=WORLDBUILD_BUILD_CHANNEL={channel}");
    println!("cargo:rustc-env=WORLDBUILD_PACKAGE_MODE={mode}");
    let integration = std::env::var_os("CARGO_FEATURE_UPDATER_INTEGRATION_TEST").is_some();
    let boundary_test = std::env::var_os("CARGO_FEATURE_UPDATER_TEST").is_some();
    if std::env::var_os("CARGO_FEATURE_COMPATIBILITY_TEST").is_some()
        && !integration
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        panic!("internal compatibility release requires the isolated installer identity");
    }
    if integration {
        if boundary_test
            || channel != "github"
            || mode != "test"
            || std::env::var("PROFILE").as_deref() != Ok("release")
        {
            panic!("integration test requires a separate release/github-test installer");
        }
        let config: serde_json::Value = serde_json::from_str(
            &std::env::var("TAURI_CONFIG").expect("explicit isolated installer config required"),
        )
        .expect("installer config JSON");
        if config["identifier"] != "com.dreamrugi.worldbuildtool.m8b.installtest"
            || config["productName"] != "Dreamrugi M8 B Install Test"
        {
            panic!("integration test cannot use production identity");
        }
    }
    if boundary_test || integration {
        if boundary_test
            && (channel != "dev"
                || mode != "dev"
                || std::env::var("PROFILE").as_deref() != Ok("debug"))
        {
            panic!("updater-test is restricted to a separate debug dev build");
        }
        for key in [
            "WORLDBUILD_UPDATER_TEST_ENDPOINT",
            "WORLDBUILD_UPDATER_TEST_PUBLIC_KEY",
            "WORLDBUILD_UPDATER_TEST_DATA_ROOT",
        ] {
            println!("cargo:rerun-if-env-changed={key}");
            let value = std::env::var(key).expect("updater-test configuration missing");
            if value.contains(['\n', '\r', '\0']) {
                panic!("invalid updater-test configuration");
            }
            if key.ends_with("ENDPOINT") && !value.starts_with("http://127.0.0.1:") {
                panic!("test endpoint must be loopback");
            }
            if key.ends_with("DATA_ROOT") && !std::path::Path::new(&value).is_absolute() {
                panic!("test data directory must be absolute");
            }
            if integration
                && key.ends_with("DATA_ROOT")
                && !value
                    .replace('\\', "/")
                    .to_ascii_lowercase()
                    .ends_with("/m8-b-install-test/data")
            {
                panic!("real installer test data must use its own isolated root");
            }
            println!("cargo:rustc-env={key}={value}");
        }
    }
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
                "svn_policy_snapshot",
                "svn_policy_initialize",
                "svn_policy_save",
                "svn_schedule_delete",
                "svn_commit",
                "svn_commit_recheck",
                "svn_cancel",
                "diagnostic_recent_events",
                "updater_status",
                "updater_check",
                "updater_download",
                "updater_cancel",
                "updater_continue",
                "updater_release",
                "updater_prepare",
                "updater_close_failed",
            ])),
    )
    .expect("Tauri app command permission generation failed")
}
