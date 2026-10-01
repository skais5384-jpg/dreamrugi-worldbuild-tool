use super::*;

#[test]
fn cancel_after_ready_discards_consent_and_bytes_and_is_idempotent() {
    let manager = Manager::new(
        std::env::temp_dir().join(uuid::Uuid::new_v4().to_string()),
        "0.2.0".into(),
    );
    let mut inner = manager.lock();
    inner.status.candidate = Some("current".into());
    inner.status.phase = "ready";
    inner.status.downloaded = 3;
    inner.status.total = Some(3);
    inner.bytes = Some(b"MZ0".to_vec());
    inner.consented = true;
    let old_epoch = inner.epoch;
    assert_eq!(cancel_download(&mut inner, "different"), Err("target"));
    assert!(inner.bytes.is_some());
    cancel_download(&mut inner, "current").unwrap();
    assert_eq!(inner.status.phase, "cancelled");
    assert!(!inner.consented && inner.bytes.is_none());
    assert!(inner.epoch > old_epoch);
    let cancelled_epoch = inner.epoch;
    cancel_download(&mut inner, "current").unwrap();
    assert_eq!(inner.epoch, cancelled_epoch);
    for phase in ["preparing", "installing", "install_failed"] {
        inner.status.phase = phase;
        assert_eq!(cancel_download(&mut inner, "current"), Err("target"));
        assert_eq!(inner.status.phase, phase);
    }
}

#[test]
fn old_deferral_file_does_not_suppress_startup() {
    let root = std::env::temp_dir().join(format!("m8-preferences-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("updater-deferred.json");
    std::fs::write(&path, br#"{"version":"0.3.0","timestamp":1}"#).unwrap();
    let manager = Manager::new(root.clone(), "0.2.0".into());
    let mut inner = manager.lock();
    inner.status.distribution = "store";
    assert!(!allowed_distribution(&inner.status));
    drop(inner);
    // 이 시험이 만든 알려진 파일과 빈 디렉터리만 정리한다.
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}

/// 통신 mock이 아닌 실제 plugin check/download/minisign 검증. 실행 입력은 소유 loopback fixture뿐이다.
#[test]
#[ignore = "start scripts/updater-test-server.mjs and set M8_UPDATER_FIXTURE"]
fn actual_plugin_signature_and_network_matrix() {
    let root = PathBuf::from(std::env::var("M8_UPDATER_FIXTURE").expect("owned fixture directory"));
    let ready: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("ready.json")).unwrap()).unwrap();
    let endpoint = ready["endpoint"]
        .as_str()
        .unwrap()
        .parse::<url::Url>()
        .unwrap();
    assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
    let key = std::fs::read_to_string(root.join("public.key.pub")).unwrap();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().plugins.0.insert("updater".into(), serde_json::json!({"pubkey":key,"requireSignedVersion":true,"dangerousInsecureTransportProtocol":true}));
    let app = tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(context)
        .unwrap();
    for (scenario, expected) in [
        ("normal", None),
        ("tampered", Some("signature")),
        ("wrong_key", Some("signature")),
        ("version_mismatch", Some("signature")),
        ("partial", Some("network")),
        ("network", Some("network")),
    ] {
        std::fs::write(
            root.join("fixture-control.json"),
            serde_json::json!({"scenario":scenario,"slow":false}).to_string(),
        )
        .unwrap();
        let result = tauri::async_runtime::block_on(async {
            let updater = app
                .updater_builder()
                .endpoints(vec![endpoint.clone()])
                .unwrap()
                .pubkey(key.clone())
                .timeout(Duration::from_secs(10))
                .target(TARGET)
                .version_comparator(|_, _| true)
                .build()
                .unwrap();
            let update = updater
                .check()
                .await
                .map_err(|e| failure(&e))?
                .expect("candidate");
            update
                .download(|_, _| {}, || {})
                .await
                .map_err(|e| failure(&e))
        });
        match expected {
            None => assert!(result.unwrap().starts_with(b"MZ")),
            Some(code) => assert_eq!(result.err(), Some(code), "{scenario}"),
        }
    }
    std::fs::write(
        root.join("fixture-control.json"),
        "{\"scenario\":\"normal\",\"slow\":true}",
    )
    .unwrap();
}
